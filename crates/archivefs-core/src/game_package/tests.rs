use super::*;

/// A minimal PARAM.SFO with UTF-8 values (test fixture builder).
pub(crate) fn sfo_bytes(pairs: &[(&str, &str)]) -> Vec<u8> {
    const HEADER: usize = 20;
    const ENTRY: usize = 16;
    let key_table_start = HEADER + pairs.len() * ENTRY;
    let mut key_table = Vec::new();
    let mut key_offsets = Vec::new();
    for (key, _) in pairs {
        key_offsets.push(key_table.len() as u16);
        key_table.extend_from_slice(key.as_bytes());
        key_table.push(0);
    }
    while key_table.len() % 4 != 0 {
        key_table.push(0);
    }
    let data_table_start = key_table_start + key_table.len();
    let (mut data, mut offsets, mut lens) = (Vec::new(), Vec::new(), Vec::new());
    for (_, value) in pairs {
        offsets.push(data.len() as u32);
        let mut raw = value.as_bytes().to_vec();
        raw.push(0);
        lens.push(raw.len() as u32);
        data.extend_from_slice(&raw);
    }
    let mut out = vec![0u8; HEADER];
    out[0..4].copy_from_slice(&[0x00, b'P', b'S', b'F']);
    out[4..8].copy_from_slice(&0x0101_u32.to_le_bytes());
    out[8..12].copy_from_slice(&(key_table_start as u32).to_le_bytes());
    out[12..16].copy_from_slice(&(data_table_start as u32).to_le_bytes());
    out[16..20].copy_from_slice(&(pairs.len() as u32).to_le_bytes());
    for i in 0..pairs.len() {
        let mut entry = [0u8; ENTRY];
        entry[0..2].copy_from_slice(&key_offsets[i].to_le_bytes());
        entry[2..4].copy_from_slice(&0x0204_u16.to_le_bytes());
        entry[4..8].copy_from_slice(&lens[i].to_le_bytes());
        entry[8..12].copy_from_slice(&lens[i].to_le_bytes());
        entry[12..16].copy_from_slice(&offsets[i].to_le_bytes());
        out.extend_from_slice(&entry);
    }
    out.extend_from_slice(&key_table);
    out.extend_from_slice(&data);
    out
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
    p
}

#[test]
fn a_cue_set_is_one_package_and_its_tracks_never_move_alone() {
    let d = tempfile::tempdir().unwrap();
    let cue = write(
        d.path(),
        "G.cue",
        "FILE \"G (Track 1).bin\" BINARY\nFILE \"G (Track 2).bin\" BINARY\n",
    );
    write(d.path(), "G (Track 1).bin", "1");
    write(d.path(), "G (Track 2).bin", "2");
    let mut r = PackageResolver::new();
    let p = r.package_of(&cue).unwrap();
    assert_eq!(
        p.shape,
        PackageShape::FileSet {
            members: vec![
                d.path().join("G (Track 1).bin"),
                d.path().join("G (Track 2).bin")
            ]
        }
    );
    let alone = r.package_of(&d.path().join("G (Track 1).bin")).unwrap_err();
    assert!(alone.contains("belongs to G.cue"), "{alone}");
}

#[test]
fn an_m3u_brings_its_discs_and_their_tracks_and_a_disc_never_moves_alone() {
    let d = tempfile::tempdir().unwrap();
    let m3u = write(
        d.path(),
        "Game.m3u",
        "#EXTM3U\nGame (Disc 1).cue\nGame (Disc 2).chd\n",
    );
    write(
        d.path(),
        "Game (Disc 1).cue",
        "FILE \"Game (Disc 1).bin\" BINARY\n",
    );
    write(d.path(), "Game (Disc 1).bin", "d1");
    write(d.path(), "Game (Disc 2).chd", "d2");
    let mut r = PackageResolver::new();
    let p = r.package_of(&m3u).unwrap();
    assert_eq!(p.paths().len(), 4);
    assert!(
        r.package_of(&d.path().join("Game (Disc 1).cue"))
            .unwrap_err()
            .contains("Game.m3u")
    );
    assert!(
        r.package_of(&d.path().join("Game (Disc 2).chd"))
            .unwrap_err()
            .contains("Game.m3u")
    );
}

#[test]
fn incomplete_shared_and_sub_folder_sets_are_refused() {
    let d = tempfile::tempdir().unwrap();
    let missing = write(d.path(), "Missing.cue", "FILE \"Gone.bin\" BINARY\n");
    let a = write(d.path(), "A.cue", "FILE \"Shared.bin\" BINARY\n");
    write(d.path(), "B.cue", "FILE \"Shared.bin\" BINARY\n");
    write(d.path(), "Shared.bin", "s");
    let sub = write(d.path(), "Sub.cue", "FILE \"tracks/T.bin\" BINARY\n");
    write(d.path(), "tracks/T.bin", "t");
    let mut r = PackageResolver::new();
    assert!(r.package_of(&missing).is_err());
    let shared = r.package_of(&a).unwrap_err();
    assert!(
        shared.contains("also used by B.cue") || shared.contains("belongs to"),
        "{shared}"
    );
    assert!(r.package_of(&sub).unwrap_err().contains("sub-folder"));
    // A plain file is its own package.
    let single = write(d.path(), "Solo.zip", "z");
    assert_eq!(
        r.package_of(&single).unwrap().shape,
        PackageShape::SingleFile
    );
}

#[test]
fn ps3_folders_are_recognised_by_their_validated_layout_only() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let game = root.join("mixed/Game");
    std::fs::create_dir_all(game.join("PS3_GAME/USRDIR")).unwrap();
    std::fs::write(game.join("PS3_GAME/USRDIR/EBOOT.BIN"), b"x").unwrap();
    // No PARAM.SFO yet: not recognised.
    assert!(find_game_folders(std::slice::from_ref(&root), 3).is_empty());
    std::fs::write(game.join("PS3_GAME/PARAM.SFO"), b"not an sfo").unwrap();
    assert!(ps3_game_folder(&game).is_none());
    std::fs::write(
        game.join("PS3_GAME/PARAM.SFO"),
        sfo_bytes(&[("TITLE_ID", "BLES01234")]),
    )
    .unwrap();
    assert_eq!(ps3_game_folder(&game).as_deref(), Some("BLES01234"));
    let found = find_game_folders(std::slice::from_ref(&root), 3);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, game);
}

#[test]
fn a_folder_layout_records_every_entry_and_refuses_links() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "a/b.bin", "12345");
    write(d.path(), "c.txt", "1");
    let layout = capture_folder_layout(d.path()).unwrap();
    assert_eq!(layout.entries.len(), 3);
    assert_eq!(layout.total_bytes, 6);
    std::os::unix::fs::symlink("c.txt", d.path().join("link")).unwrap();
    assert!(
        capture_folder_layout(d.path())
            .unwrap_err()
            .contains("link")
    );
}
