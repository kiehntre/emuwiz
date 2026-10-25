//! Focused tests for TOSEC release-pack inventory, classification, selection,
//! persistence and registration. Every fixture is a small synthetic pack in a
//! temporary directory; nothing network-facing exists in this module.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;
use crate::dat::sources::DatSourceOwnership;

const AMIGA_GAMES_FLOPPY_DAT: &str = r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>Amiga - Games - Floppy</name>
    <description>Amiga - Games - Floppy (TOSEC-v2021-01-09)</description>
    <version>2021-01-09</version>
    <author>TOSEC</author>
    <homepage>https://www.tosecdev.org/</homepage>
  </header>
  <game name="Test Game (Europe)">
    <rom name="Test Game (Europe).adf" size="4" crc="00000001" md5="00000000000000000000000000000001" sha1="0000000000000000000000000000000000000001"/>
  </game>
  <game name="Another Game (Europe)">
    <rom name="Another Game (Europe).adf" size="4" crc="00000002" md5="00000000000000000000000000000002" sha1="0000000000000000000000000000000000000002"/>
  </game>
</datafile>
"#;

fn dat_xml(name: &str, description: &str, version: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>{name}</name>
    <description>{description}</description>
    <version>{version}</version>
    <author>TOSEC</author>
    <homepage>https://www.tosecdev.org/</homepage>
  </header>
  <game name="Test Entry (Europe)">
    <rom name="Test Entry (Europe).bin" size="4" crc="0000000a" md5="0000000000000000000000000000000a" sha1="000000000000000000000000000000000000000a"/>
  </game>
</datafile>
"#
    )
}

struct PackFixture {
    _root: tempfile::TempDir,
    pack_root: PathBuf,
}

impl PackFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let pack_root = root.path().join("TOSEC_pack_2021");
        std::fs::create_dir_all(&pack_root).unwrap();
        Self {
            _root: root,
            pack_root,
        }
    }

    fn write_dat(&self, relative: &[&str], contents: &str) -> PathBuf {
        let mut path = self.pack_root.clone();
        for segment in relative {
            path.push(segment);
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// A representative synthetic official-style pack.
    fn standard() -> Self {
        let fixture = Self::new();
        fixture.write_dat(
            &["Amiga", "Amiga - Games - Floppy (TOSEC-v2021-01-09).dat"],
            AMIGA_GAMES_FLOPPY_DAT,
        );
        fixture.write_dat(
            &[
                "ZX Spectrum",
                "ZX Spectrum - Games - Tape (TOSEC-v2021-01-09).dat",
            ],
            &dat_xml(
                "ZX Spectrum - Games - Tape",
                "ZX Spectrum - Games - Tape (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        fixture.write_dat(
            &[
                "ZX Spectrum",
                "ZX Spectrum - Firmware (TOSEC-v2021-01-09).dat",
            ],
            &dat_xml(
                "ZX Spectrum - Firmware",
                "ZX Spectrum - Firmware (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        fixture.write_dat(
            &["ZX Spectrum", "ZX Spectrum - Demos (TOSEC-v2021-01-09).dat"],
            &dat_xml(
                "ZX Spectrum - Demos",
                "ZX Spectrum - Demos (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        // An unclassifiable catalogue: stays Everything Else.
        fixture.write_dat(
            &[
                "Mystery System",
                "Mystery System - Weird Stuff (TOSEC-v2021-01-09).dat",
            ],
            &dat_xml(
                "Mystery System - Weird Stuff",
                "Mystery System - Weird Stuff (TOSEC-v2021-01-09)",
                "2021-01-09",
            ),
        );
        // Non-DAT junk that must never enter the inventory.
        std::fs::write(fixture.pack_root.join("readme.txt"), "not a dat").unwrap();
        std::fs::write(
            fixture.pack_root.join("cover.png"),
            [0_u8, 1, 2, 3].as_slice(),
        )
        .unwrap();
        fixture
    }
}

fn find_dat<'a>(inventory: &'a TosecPackInventory, leaf: &str) -> &'a TosecPackDat {
    inventory
        .dats
        .iter()
        .find(|dat| dat.relative_path.to_string_lossy().contains(leaf))
        .unwrap_or_else(|| panic!("{leaf} missing from inventory"))
}

fn managed_fixture() -> (PackFixture, TosecPackInventory, BTreeSet<TosecSelectionKey>) {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut selections = BTreeSet::new();
    selections.insert(find_dat(&inventory, "Amiga - Games - Floppy").selection_key());
    (fixture, inventory, selections)
}

fn managed_store() -> (tempfile::TempDir, TosecManagedSnapshotStore) {
    let root = tempfile::tempdir().unwrap();
    let store = TosecManagedSnapshotStore::new(root.path().join("tosec-snapshots")).unwrap();
    (root, store)
}

#[test]
fn managed_snapshot_stages_without_activation_and_keeps_hashes() {
    let (_fixture, inventory, selections) = managed_fixture();
    let (_store_root, store) = managed_store();
    let candidate = store
        .stage_release_pack(&inventory, &selections, 123)
        .unwrap();
    assert!(store.active_snapshot().unwrap().is_none());
    assert_eq!(candidate.snapshot.dats.len(), 1);
    assert_eq!(candidate.snapshot.imported_unix_seconds, 123);
    assert_eq!(candidate.snapshot.dats[0].0.content_sha256.len(), 64);
    assert_eq!(candidate.validated.snapshot.sha256.len(), 64);
    let preview = store.preview_activation(&candidate).unwrap();
    assert_eq!(preview.dat_count, 1);
    assert_eq!(preview.selected_group_count, 1);
    assert!(preview.differs_from_active);
}

#[test]
fn managed_snapshot_activation_history_rollback_and_offline_load_work() {
    let (fixture, inventory, selections) = managed_fixture();
    let (_store_root, store) = managed_store();
    let first = store
        .stage_release_pack(&inventory, &selections, 123)
        .unwrap();
    let first_result = store.activate(&first, None).unwrap();
    assert_eq!(
        first_result.change.freshness,
        VerificationFreshness::NeedsRecheck
    );
    let first_hash = first_result.active.sha256.clone();

    std::fs::write(
        fixture
            .pack_root
            .join("Amiga/Amiga - Games - Floppy (TOSEC-v2021-01-09).dat"),
        AMIGA_GAMES_FLOPPY_DAT.replace("Another Game", "Changed Game"),
    )
    .unwrap();
    let second_inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let second = store
        .stage_release_pack(&second_inventory, &selections, 124)
        .unwrap();
    let second_result = store.activate(&second, Some(&first_hash)).unwrap();
    assert_eq!(
        second_result.change.freshness,
        VerificationFreshness::NeedsRecheck
    );
    assert!(store.store().list_snapshots().unwrap().len() >= 2);
    assert_eq!(
        store.freshness_for(Some(&first_hash)).unwrap(),
        VerificationFreshness::NeedsRecheck
    );

    std::fs::remove_dir_all(&fixture.pack_root).unwrap();
    let offline = store.active_snapshot().unwrap().unwrap();
    assert_eq!(offline.dats.len(), 1);
    assert_eq!(
        offline.dats[0].1,
        AMIGA_GAMES_FLOPPY_DAT
            .replace("Another Game", "Changed Game")
            .as_bytes()
    );
    let rollback = store.rollback(&first_hash).unwrap();
    assert_eq!(rollback.active.sha256, first_hash);
    assert_eq!(
        rollback.change.freshness,
        VerificationFreshness::NeedsRecheck
    );
    assert!(store.store().list_snapshots().unwrap().len() >= 2);
}

#[test]
fn managed_preview_compares_first_install_newer_same_older_and_unknown_release() {
    let (fixture, inventory, selections) = managed_fixture();
    let (_store_root, store) = managed_store();

    let first = store
        .stage_release_pack(&inventory, &selections, 123)
        .unwrap();
    assert_eq!(
        store
            .preview_activation(&first)
            .unwrap()
            .revision_comparison,
        TosecRevisionComparison::NoActiveRelease
    );
    store.activate(&first, None).unwrap();

    let dat_path = fixture
        .pack_root
        .join("Amiga/Amiga - Games - Floppy (TOSEC-v2021-01-09).dat");
    let newer_contents = AMIGA_GAMES_FLOPPY_DAT
        .replace("2021-01-09", "2022-01-09")
        .replace("Another Game", "Newer Game");
    std::fs::write(&dat_path, &newer_contents).unwrap();
    let newer_inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let newer = store
        .stage_release_pack(&newer_inventory, &selections, 124)
        .unwrap();
    assert_eq!(
        store
            .preview_activation(&newer)
            .unwrap()
            .revision_comparison,
        TosecRevisionComparison::Newer
    );
    let active_hash = store.active_record().unwrap().unwrap().sha256;
    store.activate(&newer, Some(&active_hash)).unwrap();

    let same_inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let same = store
        .stage_release_pack(&same_inventory, &selections, 125)
        .unwrap();
    assert_eq!(
        store.preview_activation(&same).unwrap().revision_comparison,
        TosecRevisionComparison::Same
    );

    let older_contents = newer_contents.replace("2022-01-09", "2020-01-09");
    std::fs::write(&dat_path, &older_contents).unwrap();
    let older_inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let older = store
        .stage_release_pack(&older_inventory, &selections, 126)
        .unwrap();
    assert_eq!(
        store
            .preview_activation(&older)
            .unwrap()
            .revision_comparison,
        TosecRevisionComparison::Older
    );

    let unknown_contents = older_contents.replace("2020-01-09", "release-a");
    std::fs::write(&dat_path, &unknown_contents).unwrap();
    let unknown_inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let unknown = store
        .stage_release_pack(&unknown_inventory, &selections, 127)
        .unwrap();
    assert_eq!(
        store
            .preview_activation(&unknown)
            .unwrap()
            .revision_comparison,
        TosecRevisionComparison::Unknown
    );
}

#[test]
fn managed_snapshot_registration_uses_immutable_materialized_bytes_after_source_disappears() {
    let (fixture, inventory, selections) = managed_fixture();
    let (store_root, store) = managed_store();
    let candidate = store
        .stage_release_pack(&inventory, &selections, 123)
        .unwrap();
    store.activate(&candidate, None).unwrap();
    let registry_path = store_root.path().join("dat-sources.toml");

    std::fs::remove_dir_all(&fixture.pack_root).unwrap();
    let outcome = store
        .register_active_snapshot_to_registry(&registry_path, 124)
        .unwrap();
    assert_eq!(outcome.registered.len(), 1);
    let config = crate::dat::sources::load_dat_sources_config_from(&registry_path).unwrap();
    let entry = config.sources.unwrap().into_iter().next().unwrap();
    assert!(entry.path.contains("materialized"));
    assert!(Path::new(&entry.path).is_file());
    assert!(!entry.path.contains("TOSEC_pack_2021"));
}

#[test]
fn managed_snapshot_rejects_path_traversal_and_symlink_escape() {
    let (fixture, mut inventory, selections) = managed_fixture();
    let (_store_root, store) = managed_store();
    inventory.dats[0].relative_path = PathBuf::from("../outside.dat");
    assert!(
        store
            .stage_release_pack(&inventory, &selections, 1)
            .is_err()
    );

    #[cfg(unix)]
    {
        let outside = fixture._root.path().join("outside.dat");
        std::fs::write(&outside, AMIGA_GAMES_FLOPPY_DAT).unwrap();
        let link = fixture.pack_root.join("link.dat");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        inventory.dats[0].relative_path = PathBuf::from("link.dat");
        assert!(
            store
                .stage_release_pack(&inventory, &selections, 1)
                .is_err()
        );
    }
}

#[test]
fn managed_snapshot_refuses_oversized_dat_before_import() {
    let root = tempfile::tempdir().unwrap();
    let pack = root.path().join("pack");
    std::fs::create_dir_all(&pack).unwrap();
    let path = pack.join("Amiga - Games - Floppy.dat");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(crate::dat::limits::DEFAULT_MAX_FILE_SIZE + 1)
        .unwrap();
    let inventory = TosecPackInventory {
        pack_root: pack,
        pack_id: "oversized".to_string(),
        dats: vec![TosecPackDat {
            relative_path: PathBuf::from("Amiga - Games - Floppy.dat"),
            raw_catalogue_name: "Amiga - Games - Floppy".to_string(),
            system: "Amiga".to_string(),
            category: TosecFriendlyCategory::Games,
            media: TosecMediaType::FloppyDisk,
            raw_category_label: "Games".to_string(),
            classification_confident: true,
            content_sha256: Some("0".repeat(64)),
        }],
        skipped: Vec::new(),
        scan_complete: true,
    };
    let (_store_root, store) = managed_store();
    let mut selections = BTreeSet::new();
    selections.insert(inventory.dats[0].selection_key());
    assert!(
        store
            .stage_release_pack(&inventory, &selections, 1)
            .is_err()
    );
}

#[test]
fn managed_snapshot_active_does_not_mean_current() {
    let (_fixture, inventory, selections) = managed_fixture();
    let (_store_root, store) = managed_store();
    let candidate = store
        .stage_release_pack(&inventory, &selections, 1)
        .unwrap();
    store.activate(&candidate, None).unwrap();
    assert_eq!(
        store.freshness_for(None).unwrap(),
        VerificationFreshness::NeedsRecheck
    );
    assert_eq!(
        store
            .freshness_for(Some(&candidate.validated.snapshot.sha256))
            .unwrap(),
        VerificationFreshness::Current
    );
}

#[test]
fn managed_snapshot_rejects_conflicting_duplicate_catalogue_names() {
    let fixture = PackFixture::new();
    let first = fixture.write_dat(
        &["one", "Amiga - Games - Floppy (TOSEC-v2021-01-09).dat"],
        AMIGA_GAMES_FLOPPY_DAT,
    );
    fixture.write_dat(
        &["two", "Amiga - Games - Floppy (TOSEC-v2021-01-09).dat"],
        &AMIGA_GAMES_FLOPPY_DAT.replace("Another Game", "Different Game"),
    );
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert_eq!(inventory.dats.len(), 2);
    assert_ne!(
        inventory.dats[0].content_sha256,
        inventory.dats[1].content_sha256
    );
    let mut selections = BTreeSet::new();
    selections.insert(inventory.dats[0].selection_key());
    let (_store_root, store) = managed_store();
    assert!(
        store
            .stage_release_pack(&inventory, &selections, 1)
            .is_err()
    );
    assert!(first.exists());
}

#[test]
fn managed_snapshot_rejects_excessive_selected_member_count() {
    let (_fixture, mut inventory, selections) = managed_fixture();
    let template = inventory.dats[0].clone();
    let target_len = MAX_TOSEC_SNAPSHOT_MEMBERS + inventory.dats.len();
    inventory.dats.resize(target_len, template);
    let (_store_root, store) = managed_store();
    assert!(
        store
            .stage_release_pack(&inventory, &selections, 1)
            .is_err()
    );
}

#[test]
fn a_small_synthetic_release_directory_is_inventoried() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert!(inventory.scan_complete);
    assert_eq!(inventory.dats.len(), 5);
    assert!(!inventory.pack_id.is_empty());
}

#[test]
fn non_dat_junk_is_ignored() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert!(
        !inventory
            .dats
            .iter()
            .any(|dat| dat.relative_path.to_string_lossy().contains("readme"))
    );
    assert!(
        !inventory
            .dats
            .iter()
            .any(|dat| dat.relative_path.to_string_lossy().contains("cover.png"))
    );
}

#[test]
fn a_symlink_inside_the_pack_is_never_followed() {
    let fixture = PackFixture::new();
    fixture.write_dat(
        &["Amiga", "Amiga - Games - Floppy (TOSEC-v2021-01-09).dat"],
        AMIGA_GAMES_FLOPPY_DAT,
    );
    // A link that points OUTSIDE the pack, disguised as a DAT and as a folder.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc/passwd.dat", fixture.pack_root.join("evil.dat")).unwrap();
        std::os::unix::fs::symlink("/etc", fixture.pack_root.join("escape-dir")).unwrap();
    }
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    // Only the real regular-file DAT is inventoried.
    assert_eq!(inventory.dats.len(), 1);
    assert!(
        inventory
            .skipped
            .iter()
            .any(|skipped| skipped.reason.contains("symbolic link"))
    );
}

#[test]
fn original_relative_paths_are_preserved_verbatim() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let amiga = find_dat(&inventory, "Amiga - Games - Floppy");
    assert_eq!(
        amiga.relative_path,
        PathBuf::from("Amiga").join("Amiga - Games - Floppy (TOSEC-v2021-01-09).dat")
    );
}

#[test]
fn raw_tosec_names_and_categories_are_preserved() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    for dat in &inventory.dats {
        assert!(!dat.raw_catalogue_name.is_empty());
        assert!(!dat.raw_category_label.is_empty());
    }
    let mystery = find_dat(&inventory, "Mystery System");
    assert_eq!(
        mystery.raw_catalogue_name,
        "Mystery System - Weird Stuff (TOSEC-v2021-01-09)"
    );
    assert_eq!(mystery.raw_category_label, "Weird Stuff");
}

#[test]
fn systems_come_from_the_catalogue_name_not_guesses() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert_eq!(find_dat(&inventory, "Games - Floppy").system, "Amiga");
    assert_eq!(
        find_dat(&inventory, "ZX Spectrum - Games - Tape").system,
        "ZX Spectrum"
    );
    assert_eq!(
        find_dat(&inventory, "Mystery System").system,
        "Mystery System"
    );
}

#[test]
fn games_tape_and_floppy_are_classified() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let tape = find_dat(&inventory, "ZX Spectrum - Games - Tape");
    assert_eq!(tape.category, TosecFriendlyCategory::Games);
    assert_eq!(tape.media, TosecMediaType::Tape);
    assert!(tape.classification_confident);

    let floppy = find_dat(&inventory, "Amiga - Games - Floppy");
    assert_eq!(floppy.category, TosecFriendlyCategory::Games);
    assert_eq!(floppy.media, TosecMediaType::FloppyDisk);
}

#[test]
fn firmware_and_demos_are_classified() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let firmware = find_dat(&inventory, "ZX Spectrum - Firmware");
    assert_eq!(
        firmware.category,
        TosecFriendlyCategory::FirmwareSystemSoftware
    );
    assert_eq!(firmware.media, TosecMediaType::Firmware);

    let demos = find_dat(&inventory, "ZX Spectrum - Demos");
    assert_eq!(demos.category, TosecFriendlyCategory::DemosScene);
}

#[test]
fn an_uncertain_category_stays_everything_else() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mystery = find_dat(&inventory, "Mystery System");
    assert_eq!(mystery.category, TosecFriendlyCategory::EverythingElse);
    assert!(!mystery.classification_confident);
}

#[test]
fn the_raw_advanced_projection_remains_available() {
    // The raw name and raw category are retained for EVERY entry, so an
    // advanced view can always group by the original TOSEC taxonomy.
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let raw_groups: BTreeSet<String> = inventory
        .dats
        .iter()
        .map(|dat| dat.raw_category_label.clone())
        .collect();
    assert!(raw_groups.contains("Games"));
    assert!(raw_groups.contains("Firmware"));
    assert!(raw_groups.contains("Weird Stuff"));
}

fn persisted_from_inventory(inventory: &TosecPackInventory) -> PersistedTosecPack {
    PersistedTosecPack {
        pack_id: inventory.pack_id.clone(),
        root_path: inventory.pack_root.clone(),
        imported_unix_seconds: 1_700_000_000,
        // Conservative default: importing enables nothing.
        selections: BTreeSet::new(),
        dats: inventory.dats.clone(),
    }
}

fn local_source(id: &str, path: &str) -> DatSourceConfigEntry {
    DatSourceConfigEntry {
        id: id.to_string(),
        display_name: "My own DAT".to_string(),
        path: path.to_string(),
        kind: DatSourceKind::File,
        ownership: DatSourceOwnership::UserLocal,
        enabled: Some(true),
        priority: Some(100),
        platform: None,
        origin: None,
        added_unix_seconds: None,
        health_state: None,
        health_last_validated_unix_seconds: None,
        health_detail: None,
        health_entry_count: None,
        health_rom_count: None,
        health_file_count: None,
        health_formats: None,
        health_observed_size_bytes: None,
        health_observed_modified_unix_seconds: None,
        health_observed_modified_subsec_nanos: None,
        health_arcade_catalogue_revisions: None,
        unknown_fields: toml::Table::new(),
    }
}

fn select_amiga_floppy(pack: &mut PersistedTosecPack) {
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
}

#[test]
fn a_user_can_enable_one_system_category_media_subset() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "ZX Spectrum".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::Tape,
    });
    let selected: Vec<&TosecPackDat> = pack.selected_dats().collect();
    assert_eq!(selected.len(), 1);
    assert!(
        selected[0]
            .relative_path
            .to_string_lossy()
            .contains("Games - Tape")
    );
}

#[test]
fn unselected_dats_are_never_registered() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert_eq!(outcome.registered.len(), 1);
    assert_eq!(outcome.failed.len(), 0);
    assert!(
        outcome.registered[0]
            .entry
            .display_name
            .contains("Amiga - Games - Floppy")
    );
}

#[test]
fn selected_release_pack_dat_gets_typed_ownership_that_survives_registry_reload() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let registry = fixture._root.path().join("dat_sources.toml");

    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert_eq!(outcome.registered.len(), 1);
    assert!(matches!(
        outcome.registered[0].entry.ownership,
        DatSourceOwnership::ImportedTosecReleasePack {
            ref pack_id,
            ref relative_path,
        } if pack_id == &pack.pack_id && relative_path == &outcome.registered[0].provenance.relative_path
    ));

    let reloaded = load_dat_sources_config_from(&registry).unwrap();
    let entry = &reloaded.sources.unwrap()[0];
    assert_eq!(entry.ownership, outcome.registered[0].entry.ownership);
}

#[test]
fn old_dat_source_config_without_ownership_stays_user_local() {
    let fixture = PackFixture::new();
    let registry = fixture._root.path().join("dat_sources.toml");
    std::fs::write(
        &registry,
        r#"[[sources]]
id = "old-local"
display_name = "Old local DAT"
path = "/tmp/old.dat"
kind = "file"
origin = "TOSEC release pack whatever"
"#,
    )
    .unwrap();
    let config = load_dat_sources_config_from(&registry).unwrap();
    assert_eq!(
        config.sources.unwrap()[0].ownership,
        DatSourceOwnership::UserLocal
    );
}

#[test]
fn deselecting_a_group_removes_only_entries_owned_by_that_exact_pack() {
    let first = PackFixture::standard();
    let first_inventory = inventory_release_pack(&first.pack_root).unwrap();
    let mut first_pack = persisted_from_inventory(&first_inventory);
    select_amiga_floppy(&mut first_pack);

    let second = PackFixture::standard();
    let second_inventory = inventory_release_pack(&second.pack_root).unwrap();
    let mut second_pack = persisted_from_inventory(&second_inventory);
    select_amiga_floppy(&mut second_pack);

    let registry = first._root.path().join("dat_sources.toml");
    apply_selection_to_registry(&first_pack, &registry, 42).unwrap();
    apply_selection_to_registry(&second_pack, &registry, 43).unwrap();

    let mut config = load_dat_sources_config_from(&registry).unwrap();
    let local = local_source("manual-tosec", "/manual/tosec.dat");
    let mut managed = local_source("managed-mame", "/managed/mame.dat");
    managed.ownership = DatSourceOwnership::EmuWizManaged;
    config.sources.as_mut().unwrap().extend([local, managed]);
    save_dat_sources_config_to(&registry, &config).unwrap();

    first_pack.selections.clear();
    let outcome = apply_selection_to_registry(&first_pack, &registry, 44).unwrap();
    assert_eq!(outcome.removed.len(), 1);
    let after = load_dat_sources_config_from(&registry).unwrap();
    let entries = after.sources.unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().any(|entry| {
        matches!(
            &entry.ownership,
            DatSourceOwnership::ImportedTosecReleasePack { pack_id, .. }
                if pack_id == &second_pack.pack_id
        )
    }));
    assert!(entries.iter().any(|entry| entry.id == "manual-tosec"));
    assert!(entries.iter().any(|entry| entry.id == "managed-mame"));
}

#[test]
fn a_deselected_release_pack_group_can_be_reselected() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let registry = fixture._root.path().join("dat_sources.toml");
    let first = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    let first_id = first.registered[0].entry.id.clone();

    pack.selections.clear();
    let removed = apply_selection_to_registry(&pack, &registry, 43).unwrap();
    assert_eq!(removed.removed.len(), 1);

    select_amiga_floppy(&mut pack);
    let reselected = apply_selection_to_registry(&pack, &registry, 44).unwrap();
    assert_eq!(reselected.registered[0].entry.id, first_id);
    let entries = load_dat_sources_config_from(&registry)
        .unwrap()
        .sources
        .unwrap();
    assert_eq!(entries.len(), 1);
}

#[test]
fn selections_survive_persistence_and_reload() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "ZX Spectrum".to_string(),
        category: TosecFriendlyCategory::FirmwareSystemSoftware,
        media: TosecMediaType::Firmware,
    });
    let path = fixture._root.path().join("tosec_release_packs.json");
    save_tosec_packs(&path, &[pack.clone()]).unwrap();
    let reloaded = load_tosec_packs(&path).unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0], pack);
    assert_eq!(reloaded[0].selections.len(), 1);
}

#[test]
fn exact_provenance_is_retained_for_registered_dats() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    let registered = &outcome.registered[0];
    assert_eq!(registered.provenance.pack_id, pack.pack_id);
    assert_eq!(
        registered.provenance.tosec_header_name,
        "Amiga - Games - Floppy"
    );
    assert_eq!(
        registered.provenance.tosec_version.as_deref(),
        Some("2021-01-09")
    );
    assert!(registered.provenance.content_sha256.is_some());
    // The registered origin line carries the same provenance.
    let origin = registered.entry.origin.clone().unwrap();
    assert!(origin.contains("TOSEC release pack"));
    assert!(origin.contains(registered.provenance.content_sha256.as_deref().unwrap()));
}

#[test]
fn a_selected_dat_feeds_the_existing_tosec_parser_and_evidence_path() {
    // Registration validates through import_tosec_dat: the bounded parser,
    // the internal TOSEC ecosystem gate and the DatIndex. Prove the selected
    // DAT passes that exact existing path with real parsed content.
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let amiga = find_dat(&inventory, "Amiga - Games - Floppy");
    let imported = crate::identity_source::tosec::import_tosec_dat(
        &fixture.pack_root.join(&amiga.relative_path),
    )
    .unwrap();
    assert_eq!(imported.system_name, "Amiga - Games - Floppy");
    assert!(!imported.index.by_crc32.is_empty());
    assert!(imported.index.lookup_crc32("00000001").len() == 1);
}

#[test]
fn the_pack_is_never_modified_by_inventory_or_registration() {
    let fixture = PackFixture::standard();
    let before: BTreeSet<(PathBuf, Vec<u8>)> = {
        let mut set = BTreeSet::new();
        fn collect(dir: &Path, set: &mut BTreeSet<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, set);
                } else {
                    set.insert((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        collect(&fixture.pack_root, &mut set);
        set
    };

    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let registry = fixture._root.path().join("dat_sources.toml");
    apply_selection_to_registry(&pack, &registry, 42).unwrap();

    let after: BTreeSet<(PathBuf, Vec<u8>)> = {
        let mut set = BTreeSet::new();
        fn collect(dir: &Path, set: &mut BTreeSet<(PathBuf, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, set);
                } else {
                    set.insert((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        collect(&fixture.pack_root, &mut set);
        set
    };
    assert_eq!(before, after, "the pack must be byte-identical after use");
}

#[test]
fn a_missing_pack_after_restart_is_reported_not_deleted() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    // Persist, then simulate the pack disappearing.
    let packs_path = fixture._root.path().join("tosec_release_packs.json");
    save_tosec_packs(&packs_path, &[pack.clone()]).unwrap();
    std::fs::remove_dir_all(&fixture.pack_root).unwrap();

    let reloaded = load_tosec_packs(&packs_path).unwrap();
    assert_eq!(
        reloaded.len(),
        1,
        "the configuration is never silently deleted"
    );
    assert_eq!(reloaded[0].availability(), PackAvailability::Missing);

    // A registration attempt against the missing pack fails honestly and
    // registers nothing.
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&reloaded[0], &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].1.contains("no longer available"));
}

#[test]
fn importing_a_pack_enables_nothing_by_default() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let pack = persisted_from_inventory(&inventory);
    assert!(pack.selections.is_empty());
    assert_eq!(pack.selected_dats().count(), 0);
    // And a registration run with no selections touches no registry entries.
    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
}

#[test]
fn existing_local_dat_sources_are_preserved_when_a_tosec_pack_registers() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    // Pre-existing user-local source in the same registry.
    let registry = fixture._root.path().join("dat_sources.toml");
    let mut sources = load_dat_sources_config_from(&registry).unwrap_or_default();
    sources
        .sources
        .get_or_insert_with(Vec::new)
        .push(local_source("user-dat", "/somewhere/user.dat"));
    save_dat_sources_config_to(&registry, &sources).unwrap();

    apply_selection_to_registry(&pack, &registry, 42).unwrap();
    let after = load_dat_sources_config_from(&registry).unwrap();
    let list = after.sources.unwrap();
    assert_eq!(list.len(), 2);
    assert!(list.iter().any(|entry| entry.id == "user-dat"));
    assert!(list.iter().any(|entry| entry.id.starts_with("tosec-")));
}

#[test]
fn an_unrelated_local_source_with_a_tosec_id_collision_is_not_replaced() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let selection = selected.selection_key();
    let generated_id = registration_id(&pack, selected);
    pack.selections.insert(selection);

    let mut sources = DatSourcesConfig::default();
    sources
        .sources
        .get_or_insert_with(Vec::new)
        .push(local_source(&generated_id, "/unrelated/user.dat"));
    let outcome = register_selected_tosec_dats(&pack, &mut sources, 42);
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.conflicts.len(), 1);
    assert!(outcome.conflicts[0].1.contains("source ID"));
    assert_eq!(sources.sources.unwrap()[0].path, "/unrelated/user.dat");
}

#[test]
fn a_tosec_looking_filename_with_non_tosec_contents_is_rejected_at_registration() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    std::fs::write(
        fixture.pack_root.join(&selected.relative_path),
        "<datafile><header><name>Not TOSEC</name></header></datafile>",
    )
    .unwrap();

    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(
        outcome.failed[0]
            .1
            .contains("does not identify itself as TOSEC")
    );
}

#[test]
fn deferred_tosec_iso_catalogues_remain_in_inventory_but_are_not_registered() {
    let fixture = PackFixture::new();
    fixture.write_dat(
        &["TOSEC-ISO - PC.dat"],
        &dat_xml("TOSEC-ISO - PC", "TOSEC-ISO - PC", "2021-01-09"),
    );
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    assert_eq!(inventory.dats.len(), 1, "inventory is read-only projection");
    let mut pack = persisted_from_inventory(&inventory);
    let selection = pack.dats[0].selection_key();
    pack.selections.insert(selection);

    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.deferred.len(), 1);
    assert!(
        outcome.deferred[0]
            .1
            .contains("deferred TOSEC ISO catalogue")
    );
}

#[test]
fn registration_revalidates_changed_selected_bytes_and_replaces_its_own_entry() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.selections.insert(TosecSelectionKey {
        system: "Amiga".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::FloppyDisk,
    });
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let inventory_digest = selected.content_sha256.clone().unwrap();
    std::fs::write(
        fixture.pack_root.join(&selected.relative_path),
        dat_xml(
            "Amiga - Games - Floppy",
            "Amiga - Games - Floppy (TOSEC-v2022-02-02)",
            "2022-02-02",
        ),
    )
    .unwrap();

    let registry = fixture._root.path().join("dat_sources.toml");
    let first = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert_eq!(first.registered.len(), 1, "failed: {:?}", first.failed);
    assert_ne!(
        first.registered[0].provenance.content_sha256.as_deref(),
        Some(inventory_digest.as_str()),
        "registration provenance must describe the bytes it actually parsed"
    );
    let first_id = first.registered[0].entry.id.clone();

    let second = apply_selection_to_registry(&pack, &registry, 43).unwrap();
    assert_eq!(second.registered.len(), 0);
    assert_eq!(second.already_registered.len(), 1);
    assert_eq!(
        load_dat_sources_config_from(&registry)
            .unwrap()
            .sources
            .unwrap()[0]
            .id,
        first_id
    );
    let saved = load_dat_sources_config_from(&registry).unwrap();
    assert_eq!(
        saved.sources.unwrap().len(),
        1,
        "a changed pack DAT must replace this pack/path's entry, not leave stale provenance"
    );
}

#[test]
fn an_exact_user_local_pack_path_is_already_satisfied_without_duplication() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let selected_path = fixture.pack_root.join(&selected.relative_path);
    let mut sources = DatSourcesConfig {
        sources: Some(vec![local_source(
            "manual-amiga",
            selected_path.to_str().unwrap(),
        )]),
        ..Default::default()
    };

    let outcome = register_selected_tosec_dats(&pack, &mut sources, 42);
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.already_registered.len(), 1);
    assert!(outcome.conflicts.is_empty());
    assert_eq!(sources.sources.unwrap().len(), 1);
}

#[test]
fn a_mixed_selection_reports_registered_already_deferred_and_conflict_separately() {
    let fixture = PackFixture::standard();
    fixture.write_dat(
        &["TOSEC-ISO - PC.dat"],
        &dat_xml("TOSEC-ISO - PC", "TOSEC-ISO - PC", "2021-01-09"),
    );
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    select_amiga_floppy(&mut pack);
    let iso = find_dat(&inventory, "TOSEC-ISO - PC");
    pack.selections.insert(iso.selection_key());
    let selected = find_dat(&inventory, "Amiga - Games - Floppy");
    let generated_id = registration_id(&pack, selected);
    pack.selections.insert(TosecSelectionKey {
        system: "ZX Spectrum".to_string(),
        category: TosecFriendlyCategory::Games,
        media: TosecMediaType::Tape,
    });
    let mut sources = DatSourcesConfig {
        sources: Some(vec![local_source(&generated_id, "/unrelated/user.dat")]),
        ..Default::default()
    };

    let outcome = register_selected_tosec_dats(&pack, &mut sources, 42);
    assert_eq!(outcome.registered.len(), 1);
    assert_eq!(outcome.already_registered.len(), 0);
    assert_eq!(outcome.deferred.len(), 1);
    assert_eq!(outcome.conflicts.len(), 1);
    assert!(outcome.failed.is_empty());
}

#[test]
fn malicious_persisted_relative_path_cannot_register_outside_the_pack() {
    let fixture = PackFixture::standard();
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut pack = persisted_from_inventory(&inventory);
    pack.dats[0].relative_path = PathBuf::from("..").join("outside.dat");
    let selection = pack.dats[0].selection_key();
    pack.selections.insert(selection);

    let registry = fixture._root.path().join("dat_sources.toml");
    let outcome = apply_selection_to_registry(&pack, &registry, 42).unwrap();
    assert!(outcome.registered.is_empty());
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].1.contains("safe relative path"));
}

#[test]
fn malformed_persisted_pack_config_is_reported_not_silently_forgotten() {
    let fixture = PackFixture::new();
    let config = fixture._root.path().join("tosec_release_packs.json");
    std::fs::write(&config, "{ definitely not json").unwrap();
    assert!(load_tosec_packs(&config).is_err());
}

// ---------------------------------------------------------------------------
// Compilations qualifier + bracketed media tokens. Names are representative
// strings copied from the real TOSEC pack's filenames (no DAT content).
// ---------------------------------------------------------------------------

fn class(name: &str) -> TosecClassification {
    classify_tosec_catalogue_name(name)
}

#[test]
fn ordinary_system_category_media_names_are_unchanged() {
    let c = class("Commodore Amiga - Games - Floppy (TOSEC-v2021-01-09)");
    assert_eq!(c.system, "Commodore Amiga");
    assert_eq!(c.category, TosecFriendlyCategory::Games);
    assert_eq!(c.media, TosecMediaType::FloppyDisk);
    assert_eq!(c.qualifier, None);
    assert_eq!(c.raw_category_label, "Games");
    assert!(c.confident);
}

#[test]
fn compilations_is_a_qualifier_not_part_of_the_system() {
    for (name, system, category, media) in [
        (
            "Sinclair ZX Spectrum - Compilations - Games - [TZX] (TOSEC-v2020-02-18_CM)",
            "Sinclair ZX Spectrum",
            TosecFriendlyCategory::Games,
            TosecMediaType::Tape,
        ),
        (
            "Commodore C64 - Compilations - Games - [D64] (TOSEC-v2021-01-01_CM)",
            "Commodore C64",
            TosecFriendlyCategory::Games,
            TosecMediaType::FloppyDisk,
        ),
        (
            "Apple II - Compilations - Games - [DSK] (TOSEC-v2024-07-03_CM)",
            "Apple II",
            TosecFriendlyCategory::Games,
            TosecMediaType::FloppyDisk,
        ),
        (
            "Atari ST - Compilations - Games - [ST] (TOSEC-v2023-11-07_CM)",
            "Atari ST",
            TosecFriendlyCategory::Games,
            TosecMediaType::FloppyDisk,
        ),
        (
            "Amstrad CPC - Compilations - Games - [CDT] (TOSEC-v2025-01-15_CM)",
            "Amstrad CPC",
            TosecFriendlyCategory::Games,
            TosecMediaType::Tape,
        ),
        (
            "Apple II - Compilations - Educational - [WOZ] (TOSEC-v2024-01-15_CM)",
            "Apple II",
            TosecFriendlyCategory::EducationalProductivity,
            TosecMediaType::FloppyDisk,
        ),
    ] {
        let c = class(name);
        assert_eq!(c.system, system, "{name}");
        assert_eq!(c.category, category, "{name}");
        assert_eq!(c.media, media, "{name}");
        assert_eq!(c.qualifier.as_deref(), Some("Compilations"), "{name}");
        // The raw label keeps the qualifier verbatim.
        assert!(
            c.raw_category_label.starts_with("Compilations - "),
            "{name}"
        );
        assert!(c.confident, "{name}");
        // A compilation DAT and an ordinary one share the same system group key.
        let plain = class(&name.replace(" - Compilations", ""));
        assert_eq!(plain.system, c.system, "{name}");
    }
}

#[test]
fn compilations_without_a_category_keeps_the_system_clean_and_records_the_qualifier() {
    for name in [
        "Atari 2600 & VCS - Compilations (TOSEC-v2022-12-21_CM)",
        "Acorn BBC - Compilations - [SSD] (TOSEC-v2013-10-16_CM)",
        "Commodore Amiga - Compilations - Various (TOSEC-v2023-06-12_CM)",
    ] {
        let c = class(name);
        assert!(!c.system.contains("Compilations"), "{name}: {}", c.system);
        assert_eq!(c.qualifier.as_deref(), Some("Compilations"), "{name}");
        assert!(!c.confident, "{name}");
        assert_eq!(c.category, TosecFriendlyCategory::EverythingElse);
    }
}

#[test]
fn raw_catalogue_name_stays_byte_identical_for_compilation_dats() {
    let raw = "Sinclair ZX Spectrum - Compilations - Games - [TZX] (TOSEC-v2020-02-18_CM)";
    let fixture = PackFixture::new();
    fixture.write_dat(
        &["ZX", &format!("{raw}.dat")],
        &dat_xml(raw, raw, "2020-02-18"),
    );
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let dat = &inventory.dats[0];
    assert_eq!(dat.raw_catalogue_name, raw);
    assert_eq!(dat.system, "Sinclair ZX Spectrum");
    assert_eq!(dat.raw_category_label, "Compilations - Games");
    assert_eq!(dat.media, TosecMediaType::Tape);
}

#[test]
fn compilations_in_any_other_position_is_not_guessed() {
    for (name, system) in [
        // Not adjacent to the category: nothing is stripped.
        (
            "Sinclair ZX Spectrum - Compilations - Extras - Games (TOSEC-v1)",
            "Sinclair ZX Spectrum - Compilations - Extras",
        ),
        // Compilations as the leading (system) segment.
        ("Compilations - Games (TOSEC-v1)", "Compilations"),
        // Only the exact qualifier segment; a word inside a longer name is not it.
        ("Foo Compilations - Games (TOSEC-v1)", "Foo Compilations"),
        // Exact spelling only: no case folding.
        (
            "Commodore C64 - compilations - Games (TOSEC-v1)",
            "Commodore C64 - compilations",
        ),
        (
            "Commodore C64 - Compilation - Games (TOSEC-v1)",
            "Commodore C64 - Compilation",
        ),
    ] {
        let c = class(name);
        assert_eq!(c.system, system, "{name}");
        assert_eq!(c.qualifier, None, "{name}");
    }
}

#[test]
fn a_neighbouring_unknown_branch_is_not_stripped_with_compilations() {
    let c = class("Apple II - Hard Disks - Games (TOSEC-v2023-08-29_CM)");
    assert_eq!(c.system, "Apple II - Hard Disks");
    assert_eq!(c.qualifier, None);
    let c = class("Apple II - Hard Disks - Compilations - Games - [HDV] (TOSEC-v2025-01-15_CM)");
    // Only the exact qualifier goes; the unknown branch stays in the system.
    assert_eq!(c.system, "Apple II - Hard Disks");
    assert_eq!(c.qualifier.as_deref(), Some("Compilations"));
    assert_eq!(c.media, TosecMediaType::HardDisk);
}

#[test]
fn reviewed_bracketed_media_tokens_map_to_existing_media_types() {
    for (name, media) in [
        (
            "Commodore C64 - Games - [D64] (TOSEC-v1)",
            TosecMediaType::FloppyDisk,
        ),
        (
            "Atari 8bit - Games - [ATR] (TOSEC-v1)",
            TosecMediaType::FloppyDisk,
        ),
        (
            "Apple II - Games - [WOZ] (TOSEC-v1)",
            TosecMediaType::FloppyDisk,
        ),
        (
            "Sinclair ZX Spectrum - Games - [TZX] (TOSEC-v1)",
            TosecMediaType::Tape,
        ),
        (
            "Commodore C64 - Games - [TAP] (TOSEC-v1)",
            TosecMediaType::Tape,
        ),
        (
            "Amstrad CPC - Games - [CDT] (TOSEC-v1)",
            TosecMediaType::Tape,
        ),
        (
            "Apple II - Hard Disks - Applications - [HDV] (TOSEC-v1)",
            TosecMediaType::HardDisk,
        ),
        (
            "Atari 8bit - Games - [CAR] (TOSEC-v1)",
            TosecMediaType::Cartridge,
        ),
        (
            "Sinclair ZX Spectrum - Games - [Z80] (TOSEC-v1)",
            TosecMediaType::Snapshot,
        ),
        (
            "Amstrad CPC - Games - [ROM] (TOSEC-v1)",
            TosecMediaType::Rom,
        ),
        // Shapes where something sits between the category and the token.
        (
            "Apple II - Games - Save Disks - [DSK] (TOSEC-v1)",
            TosecMediaType::FloppyDisk,
        ),
        // No recognised category: the token still names the medium.
        (
            "Acorn BBC - Operating Systems - [SSD] (TOSEC-v1)",
            TosecMediaType::FloppyDisk,
        ),
        // ASCII case-insensitive, like the existing keyword matching.
        (
            "Commodore C64 - Games - [d64] (TOSEC-v1)",
            TosecMediaType::FloppyDisk,
        ),
    ] {
        assert_eq!(class(name).media, media, "{name}");
    }
}

#[test]
fn unreviewed_ambiguous_and_malformed_tokens_stay_other() {
    for token in [
        // Not a reviewed medium: program files, archives, ambiguous containers.
        "[PRG]",
        "[BIN]",
        "[P00]",
        "[LNX]",
        "[Z64]",
        "[2MG]",
        "[PO]",
        "[WAV]",
        "[IMG]",
        "[CRT]",
        "[SFX]",
        "[Multipart]",
        "[RAW]",
        "[XYZZY]",
        // Optical tokens belong to the deferred TOSEC-ISO catalogues.
        "[ISO]",
        "[CUE]",
        "[BIN-CUE]",
        "[CDI]",
        // Malformed bracket syntax.
        "[D64",
        "D64]",
        "[D64]x",
        "x[D64]",
        "[]",
        "[[D64]]",
        "[D64][D64]",
        "[ D64 ]",
        "( D64 )",
    ] {
        let name = format!("Commodore C64 - Games - {token} (TOSEC-v1)");
        assert_eq!(class(&name).media, TosecMediaType::Other, "{token}");
    }
    // Conflicting tokens fail closed.
    assert_eq!(
        class("X - Games - [D64] - [TZX] (TOSEC-v1)").media,
        TosecMediaType::Other
    );
}

#[test]
fn bracket_tokens_never_assign_a_platform_or_change_the_system() {
    // The system is only ever the catalogue name's own system text.
    for (name, system) in [
        ("Nintendo 3DS - Games - [D64] (TOSEC-v1)", "Nintendo 3DS"),
        (
            "Sinclair ZX Spectrum - Games - [D64] (TOSEC-v1)",
            "Sinclair ZX Spectrum",
        ),
        ("Commodore C64 - Games - [TZX] (TOSEC-v1)", "Commodore C64"),
    ] {
        let c = class(name);
        assert_eq!(c.system, system, "{name}");
    }
}

#[test]
fn media_keywords_and_tokens_must_agree_and_unknown_formats_stay_other() {
    // Agreeing keyword and token: unchanged.
    assert_eq!(
        class("Sinclair ZX Spectrum - Games - Tape - [TZX] (TOSEC-v1)").media,
        TosecMediaType::Tape
    );
    // A keyword that disagrees with the token is conflicting evidence: Other.
    assert_eq!(
        class("Commodore C64 - Games - Tape - [D64] (TOSEC-v1)").media,
        TosecMediaType::Other
    );
    // An explicit unreviewed format prevents guessing from the Firmware category.
    assert_eq!(
        class("Acorn BBC - Firmware - [BIN] (TOSEC-v1)").media,
        TosecMediaType::Other
    );
}

#[test]
fn bracket_media_tokens_agree_with_the_content_registry() {
    use crate::ingestion::content_registry::{ContentKind, content_kind_for_extension};
    for (tokens, media) in BRACKET_MEDIA_TOKENS {
        for token in *tokens {
            let Some(kind) = content_kind_for_extension(&token.to_ascii_lowercase()) else {
                continue; // not registered: nothing to contradict
            };
            let ok = match media {
                TosecMediaType::FloppyDisk => {
                    matches!(kind, ContentKind::ComputerDisk | ContentKind::AmigaImage)
                }
                TosecMediaType::Tape => kind == ContentKind::TapeImage,
                TosecMediaType::Snapshot => kind == ContentKind::MachineSnapshot,
                TosecMediaType::Cartridge => kind == ContentKind::RomCartridge,
                TosecMediaType::HardDisk => kind == ContentKind::ComputerDisk,
                TosecMediaType::Rom => kind == ContentKind::RomCartridge,
                other => panic!("unexpected mapped media {other:?}"),
            };
            assert!(
                ok,
                "[{token}] -> {media:?} contradicts the registry ({kind:?})"
            );
        }
    }
    // A mapped token must not be an optical-disc format.
    for (tokens, _) in BRACKET_MEDIA_TOKENS {
        for token in *tokens {
            if let Some(kind) = content_kind_for_extension(&token.to_ascii_lowercase()) {
                assert_ne!(kind, ContentKind::DiscImage, "[{token}]");
            }
        }
    }
}

#[test]
fn deferred_tosec_iso_and_pix_names_are_not_reinterpreted_by_the_new_rules() {
    // TOSEC-ISO catalogue shapes: optical tokens stay Other; nothing optical is mapped.
    for name in [
        "3DO 3DO Interactive Multiplayer - Applications - [IMG] (TOSEC-v2017-09-15_CM)",
        "Sony PlayStation - Games - [ISO] (TOSEC-v2012-07-05_CM)",
        "Sega Dreamcast - Games - [GDI] (TOSEC-v2012-07-05_CM)",
    ] {
        assert_eq!(class(name).media, TosecMediaType::Other, "{name}");
    }
    // The deferral itself is by catalogue kind, covered by
    // `deferred_tosec_iso_catalogues_remain_in_inventory_but_are_not_registered`.
}

/// Representative real-pack shapes with their expected projection, captured
/// from the 1,449-DAT pack's filenames. Source-controlled so the normal suite
/// never depends on the private pack path.
#[test]
fn captured_real_pack_shapes_project_as_expected() {
    use TosecFriendlyCategory as C;
    use TosecMediaType as M;
    let cases: &[(&str, &str, C, M, Option<&str>)] = &[
        (
            "Commodore C64 - Games - [D64] (TOSEC-v2025-01-15_CM)",
            "Commodore C64",
            C::Games,
            M::FloppyDisk,
            None,
        ),
        (
            "Commodore C64 - Compilations - Games - [TAP] (TOSEC-v2021-01-01_CM)",
            "Commodore C64",
            C::Games,
            M::Tape,
            Some("Compilations"),
        ),
        (
            "Apple II - Compilations - Applications - [DSK] (TOSEC-v2024-07-03_CM)",
            "Apple II",
            C::EducationalProductivity,
            M::FloppyDisk,
            Some("Compilations"),
        ),
        (
            "Apple IIGS - Hard Disks - Compilations - Games - [HDV] (TOSEC-v2023-08-29_CM)",
            "Apple IIGS - Hard Disks",
            C::Games,
            M::HardDisk,
            Some("Compilations"),
        ),
        (
            "Atari 8bit - Games - [CAR] (TOSEC-v2025-01-15_CM)",
            "Atari 8bit",
            C::Games,
            M::Cartridge,
            None,
        ),
        (
            "Sinclair ZX Spectrum - Games - [SNA] (TOSEC-v2020-02-18_CM)",
            "Sinclair ZX Spectrum",
            C::Games,
            M::Snapshot,
            None,
        ),
        (
            "Commodore C64 - Games - [PRG] (TOSEC-v2025-01-15_CM)",
            "Commodore C64",
            C::Games,
            M::Other,
            None,
        ),
        (
            "Atari Jaguar - Homebrew - Games (TOSEC-v2025-01-15_CM)",
            "Atari Jaguar - Homebrew",
            C::Games,
            M::Other,
            None,
        ),
    ];
    for (name, system, category, media, qualifier) in cases {
        let c = class(name);
        assert_eq!(&c.system, system, "{name}");
        assert_eq!(&c.category, category, "{name}");
        assert_eq!(&c.media, media, "{name}");
        assert_eq!(c.qualifier.as_deref(), *qualifier, "{name}");
    }
}

/// Read-only, filename-only probe of a real extracted TOSEC pack. Never part of
/// the normal suite: set `EMUWIZ_TOSEC_PACK_PROBE` to the pack's `TOSEC` folder
/// and run with `--ignored --nocapture`. Reads directory entries only (no DAT
/// contents, no hashing) and prints classification metrics.
#[test]
#[ignore = "reads a private real TOSEC pack; set EMUWIZ_TOSEC_PACK_PROBE"]
fn real_pack_filename_probe() {
    use std::collections::BTreeMap;
    let Ok(root) = std::env::var("EMUWIZ_TOSEC_PACK_PROBE") else {
        return;
    };
    fn collect(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
            let path = entry.path();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                collect(&path, out);
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "dat") {
                out.push(path.file_stem().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut names: Vec<String> = Vec::new();
    collect(Path::new(&root), &mut names);
    names.sort();
    let mut other_dats = 0;
    let mut groups: BTreeSet<TosecSelectionKey> = BTreeSet::new();
    let mut other_groups: BTreeSet<TosecSelectionKey> = BTreeSet::new();
    let mut tokens: BTreeMap<String, (usize, BTreeSet<&'static str>)> = BTreeMap::new();
    let mut shapes: BTreeMap<String, usize> = BTreeMap::new();
    let mut swallowed = 0;
    let mut unconfident = 0;
    for name in &names {
        let class = classify_tosec_catalogue_name(name);
        let key = TosecSelectionKey {
            system: class.system.clone(),
            category: class.category,
            media: class.media,
        };
        if class.media == TosecMediaType::Other {
            other_dats += 1;
            other_groups.insert(key.clone());
        }
        if class.system.contains(" - ") || class.system.to_ascii_lowercase().contains("compilation")
        {
            swallowed += 1;
        }
        if !class.confident {
            unconfident += 1;
        }
        groups.insert(key);
        let cleaned = strip_tosec_version_marker(name);
        let mut shape = Vec::new();
        for (i, seg) in cleaned.split(" - ").map(str::trim).enumerate() {
            let label = if i == 0 {
                "SYS"
            } else if seg == "Compilations" {
                "COMP"
            } else if category_match(seg).is_some() {
                "CAT"
            } else if seg.starts_with('[') && seg.ends_with(']') {
                let entry = tokens.entry(seg.to_string()).or_default();
                entry.0 += 1;
                entry.1.insert(class.media.label());
                "[TOKEN]"
            } else if keyword_match(seg, MEDIA_KEYWORDS).is_some() {
                "MEDIAWORD"
            } else {
                "OTHER"
            };
            shape.push(label);
        }
        *shapes.entry(shape.join(" - ")).or_default() += 1;
    }
    let mut systems: BTreeMap<String, usize> = BTreeMap::new();
    let mut other_causes: BTreeMap<String, usize> = BTreeMap::new();
    for name in &names {
        let class = classify_tosec_catalogue_name(name);
        *systems.entry(class.system.clone()).or_default() += 1;
        if class.media == TosecMediaType::Other {
            let cleaned = strip_tosec_version_marker(name);
            let cause = cleaned
                .split(" - ")
                .map(str::trim)
                .find_map(|seg| bracket_token(seg).map(|t| format!("unmapped token [{t}]")))
                .unwrap_or_else(|| "no bracket token".to_string());
            *other_causes.entry(cause).or_default() += 1;
        }
    }
    eprintln!("PROBE distinct_systems={}", systems.len());
    for (system, count) in &systems {
        if system.contains(" - ") {
            eprintln!("PROBE branch_system {count:>4}  {system}");
        }
    }
    let mut causes: Vec<_> = other_causes.iter().collect();
    causes.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (cause, count) in causes.iter().take(25) {
        eprintln!("PROBE other_cause {count:>4}  {cause}");
    }
    eprintln!("PROBE total_dats={}", names.len());
    eprintln!("PROBE distinct_groups={}", groups.len());
    eprintln!("PROBE other_media_dats={other_dats}");
    eprintln!("PROBE other_media_groups={}", other_groups.len());
    eprintln!("PROBE system_swallowed_or_compilation_dats={swallowed}");
    eprintln!("PROBE unconfident_dats={unconfident}");
    for (shape, count) in &shapes {
        eprintln!("PROBE shape {count:>5}  {shape}");
    }
    let mut ordered: Vec<_> = tokens.iter().collect();
    ordered.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(b.0)));
    for (token, (count, media)) in ordered {
        eprintln!("PROBE token {count:>5}  {token}  -> {media:?}");
    }
}

#[test]
fn every_explicit_media_signal_must_agree_before_category_fallback() {
    for suffix in [
        "[D64] - [TZX]",
        "Tape - Disk - [TAP]",
        "Tape - Disk",
        "[D64] - [XYZZY]",
        "[XYZZY] - [D64]",
        "[D64] - [D64",
        "[D64][TAP]",
    ] {
        for category in ["Games", "Firmware", "Unrecognised Category"] {
            let raw = format!("Commodore C64 - {category} - {suffix} (TOSEC-v1)");
            assert_eq!(class(&raw).media, TosecMediaType::Other, "{raw}");
        }
    }
    for (suffix, media) in [
        ("[D64] - [D71]", TosecMediaType::FloppyDisk),
        ("Disk - [ADF] - Floppy", TosecMediaType::FloppyDisk),
        ("[TAP] - [TZX] - Tape", TosecMediaType::Tape),
    ] {
        let raw = format!("Any System - Games - {suffix} (TOSEC-v1)");
        assert_eq!(class(&raw).media, media, "{raw}");
    }
}

#[test]
fn invalid_or_ambiguous_system_text_never_becomes_platform_authority() {
    use crate::dat::tosec_system_projection::{PlatformForTosecSystem, platform_for_tosec_system};
    for raw in [
        "",
        "Games - [D64]",
        " - Games - [D64]",
        " - Compilations - Games - [D64]",
    ] {
        let c = class(raw);
        assert!(!c.confident, "{raw:?}");
        assert_eq!(
            platform_for_tosec_system(&c.system),
            PlatformForTosecSystem::Unknown
        );
        assert_eq!(c.qualifier, None);
    }
    for system in [
        "Acorn",
        "Commodore",
        "Commodore C64DTV",
        "Foo Compilations",
        "Unknown System",
    ] {
        let raw = format!("{system} - Compilations - Games - [D64]");
        let c = class(&raw);
        assert_eq!(c.system, system);
        assert_eq!(
            platform_for_tosec_system(&c.system),
            PlatformForTosecSystem::Unknown
        );
    }
}

#[test]
fn shared_media_formats_preserve_each_reviewed_parent_platform() {
    use crate::dat::tosec_system_projection::{PlatformForTosecSystem, platform_for_tosec_system};
    for (system, format, platform) in [
        ("Commodore C64", "D64", "Commodore 64"),
        ("Commodore VIC20", "D64", "VIC-20"),
        ("Acorn BBC", "SSD", "BBC Micro"),
        ("Acorn Electron", "SSD", "Acorn Electron"),
        ("Amstrad CPC", "DSK", "Amstrad CPC"),
        ("Apple II", "DSK", "Apple II"),
    ] {
        let c = class(&format!(
            "{system} - Compilations - Games - [{format}] (TOSEC-v1)"
        ));
        assert_eq!(c.system, system);
        assert_eq!(c.media, TosecMediaType::FloppyDisk);
        assert_eq!(
            platform_for_tosec_system(&c.system),
            PlatformForTosecSystem::Platform(platform)
        );
    }
}

#[test]
fn a_corrected_inventory_does_not_rewrite_persisted_classification_or_evidence() {
    let fixture = PackFixture::new();
    let raw = "Commodore C64 - Compilations - Games - [D64] (TOSEC-v2021-01-01_CM)";
    let bytes = dat_xml(raw, raw, "2021-01-01");
    let path = fixture.write_dat(&[&format!("{raw}.dat")], &bytes);
    let inventory = inventory_release_pack(&fixture.pack_root).unwrap();
    let mut legacy = persisted_from_inventory(&inventory);
    legacy.dats[0].system = "Commodore C64 - Compilations".into();
    legacy.dats[0].media = TosecMediaType::Other;
    legacy.dats[0].raw_category_label = "Games".into();
    legacy.selections.insert(legacy.dats[0].selection_key());
    let config = fixture._root.path().join("packs.json");
    save_tosec_packs(&config, &[legacy.clone()]).unwrap();
    let config_bytes = std::fs::read(&config).unwrap();
    let fresh = inventory_release_pack(&fixture.pack_root).unwrap();
    assert_eq!(fresh.dats[0].system, "Commodore C64");
    assert_eq!(fresh.dats[0].media, TosecMediaType::FloppyDisk);
    assert_eq!(fresh.dats[0].raw_catalogue_name, raw);
    assert_eq!(fresh.dats[0].relative_path, legacy.dats[0].relative_path);
    assert_eq!(fresh.dats[0].content_sha256, legacy.dats[0].content_sha256);
    assert_eq!(std::fs::read(path).unwrap(), bytes.as_bytes());
    assert_eq!(load_tosec_packs(&config).unwrap(), vec![legacy]);
    assert_eq!(std::fs::read(config).unwrap(), config_bytes);
}
