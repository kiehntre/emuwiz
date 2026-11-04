//! Loose Dreamcast GDI discs and saved DAT identity, on synthetic files and a
//! disposable database. No real disc, DAT or library is touched.
use super::*;

/// A `.gdi` descriptor is a few lines of text naming its tracks: a catalogue
/// match on it alone proves nothing about the tracks. Saved library identity
/// can only attach to a library entry, and the scan does not catalogue a
/// loose `.gdi` (only a PlayStation `.cue` is, after its sheet and every
/// track pass `ps1_cue_disc`). So no descriptor-only verified identity can
/// be saved for one.
///
/// If GDI cataloguing is added, this fails on purpose: the audit must then
/// judge the descriptor and every track together, as
/// `audit_loose_cue_discs` does for a sheet, before any GDI row can be
/// verified.
#[test]
fn a_loose_gdi_is_not_a_library_entry_so_no_descriptor_only_identity_can_be_saved() {
    let root = tempfile::tempdir().unwrap();
    let games = root.path().join("games").join("dreamcast").join("orbit");
    fs::create_dir_all(&games).unwrap();
    fs::write(
        games.join("orbit.gdi"),
        "3\n1 0 4 2352 track01.bin 0\n2 150 0 2352 track02.raw 0\n3 45000 4 2352 track03.bin 0\n",
    )
    .unwrap();
    for (name, sectors) in [("track01.bin", 2), ("track02.raw", 3), ("track03.bin", 4)] {
        fs::write(games.join(name), vec![1_u8; 2352 * sectors]).unwrap();
    }
    let config = crate::Config {
        source_folders: vec![root.path().join("games")],
        mount_root: root.path().join("mount"),
        ratarmount_bin: "ratarmount".into(),
        master_rom_root: None,
    };
    let mut database = Database::open_or_create(root.path().join("library.sqlite3")).unwrap();
    crate::scan_and_persist(&mut database, &config, "test").unwrap();
    let rows: Vec<_> = database
        .load_archives()
        .unwrap()
        .into_iter()
        .map(|archive| archive.relative_path)
        .collect();
    assert!(
        rows.is_empty(),
        "a loose GDI release became library rows {rows:?}; whole-disc DAT verification \
         for GDI must exist before that"
    );
}
