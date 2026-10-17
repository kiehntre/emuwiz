use super::*;
use std::fs;

const SECTOR: usize = 2352;

fn folder() -> PathBuf {
    tempfile::tempdir().unwrap().keep()
}

fn write_bin(dir: &Path, name: &str, sectors: usize, fill: u8) {
    fs::write(dir.join(name), vec![fill; sectors * SECTOR]).unwrap();
}

fn write_cue(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn single_and_multi_track_discs_are_one_entry_with_their_tracks_as_companions() {
    let dir = folder();
    write_bin(&dir, "Game (Track 1).bin", 20, 1);
    let single = write_cue(
        &dir,
        "Game.cue",
        "FILE \"Game (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
    );
    let disc = inspect_ps1_cue_disc(&single).unwrap();
    assert_eq!(disc.companions, [dir.join("Game (Track 1).bin")]);
    assert_eq!(disc.track_count, 1);

    let multi = folder();
    write_bin(&multi, "Multi (Track 1).bin", 20, 1);
    write_bin(&multi, "Multi (Track 2).bin", 30, 2);
    write_bin(&multi, "Multi (Track 3).bin", 30, 3);
    let cue = write_cue(
        &multi,
        "Multi.cue",
        "FILE \"Multi (Track 1).bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n\
         FILE \"Multi (Track 2).bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:00:10\n\
         FILE \"Multi (Track 3).bin\" BINARY\n  TRACK 03 AUDIO\n    PREGAP 00:02:00\n    INDEX 01 00:00:00\n",
    );
    let disc = inspect_ps1_cue_disc(&cue).unwrap();
    assert_eq!(disc.track_count, 3);
    assert_eq!(disc.companions.len(), 3);
}

#[test]
fn index_00_pregap_inside_the_data_track_file_is_understood() {
    let dir = folder();
    write_bin(&dir, "Pregap.bin", 40, 1);
    let cue = write_cue(
        &dir,
        "Pregap.cue",
        "FILE \"Pregap.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 00 00:00:00\n    INDEX 01 00:00:15\n",
    );
    assert!(inspect_ps1_cue_disc(&cue).is_ok());
    // INDEX 00 at or after INDEX 01 is not a layout the parser accepts.
    let bad = write_cue(
        &dir,
        "Backwards.cue",
        "FILE \"Pregap.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 00 00:00:20\n    INDEX 01 00:00:15\n",
    );
    assert!(inspect_ps1_cue_disc(&bad).is_err());
}

#[test]
fn missing_unsafe_and_linked_companions_are_refused() {
    let dir = folder();
    let missing = write_cue(
        &dir,
        "Missing.cue",
        "FILE \"Missing.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
    );
    assert!(matches!(
        inspect_ps1_cue_disc(&missing),
        Err(Ps1CueRefusal::MissingCompanion(_))
    ));

    let outside = folder();
    write_bin(&outside, "Elsewhere.bin", 10, 1);
    let inner = dir.join("inner");
    fs::create_dir_all(&inner).unwrap();
    write_bin(&dir, "Up.bin", 10, 1);
    let traversal = write_cue(
        &inner,
        "Traversal.cue",
        "FILE \"../Up.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
    );
    assert!(matches!(
        inspect_ps1_cue_disc(&traversal),
        Err(Ps1CueRefusal::UnsafeCompanion(_))
    ));
    let absolute = write_cue(
        &inner,
        "Absolute.cue",
        &format!(
            "FILE \"{}\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
            outside.join("Elsewhere.bin").display()
        ),
    );
    assert!(matches!(
        inspect_ps1_cue_disc(&absolute),
        Err(Ps1CueRefusal::UnsafeCompanion(_))
    ));

    let linked = folder();
    write_bin(&linked, "Real.bin", 10, 1);
    std::os::unix::fs::symlink(linked.join("Real.bin"), linked.join("Link.bin")).unwrap();
    let cue = write_cue(
        &linked,
        "Linked.cue",
        "FILE \"Link.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
    );
    assert!(inspect_ps1_cue_disc(&cue).is_err());

    let sheet_link = folder();
    write_bin(&sheet_link, "Disc.bin", 10, 1);
    let real = write_cue(
        &sheet_link,
        "Real.cue.txt",
        "FILE \"Disc.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
    );
    std::os::unix::fs::symlink(&real, sheet_link.join("Linked.cue")).unwrap();
    assert!(matches!(
        inspect_ps1_cue_disc(&sheet_link.join("Linked.cue")),
        Err(Ps1CueRefusal::Malformed(_))
    ));
}

#[test]
fn two_sheets_claiming_one_track_are_both_refused() {
    let dir = folder();
    write_bin(&dir, "Shared.bin", 10, 1);
    let body = "FILE \"Shared.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n";
    let first = write_cue(&dir, "First.cue", body);
    let second = write_cue(&dir, "Second.cue", body);
    for cue in [first, second] {
        assert!(matches!(
            inspect_ps1_cue_disc(&cue),
            Err(Ps1CueRefusal::SharedCompanion(_))
        ));
    }
}

#[test]
fn layouts_that_are_not_a_playstation_disc_are_refused() {
    let dir = folder();
    let cases = [
        (
            "Cooked.cue",
            "FILE \"A.bin\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n",
        ),
        (
            "AudioFirst.cue",
            "FILE \"A.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\nFILE \"B.bin\" BINARY\n  TRACK 02 MODE2/2352\n    INDEX 01 00:00:00\n",
        ),
        (
            "TwoData.cue",
            "FILE \"A.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\nFILE \"B.bin\" BINARY\n  TRACK 02 MODE2/2352\n    INDEX 01 00:00:00\n",
        ),
        (
            "Wave.cue",
            "FILE \"A.bin\" WAVE\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
        ),
    ];
    for (name, body) in cases {
        let own = folder();
        write_bin(&own, "A.bin", 10, 1);
        write_bin(&own, "B.bin", 10, 2);
        let cue = write_cue(&own, name, body);
        let refusal = inspect_ps1_cue_disc(&cue).unwrap_err();
        assert!(
            matches!(
                refusal,
                Ps1CueRefusal::UnsupportedLayout(_) | Ps1CueRefusal::Malformed(_)
            ),
            "{name}: {refusal:?}"
        );
    }
    // A partial sector is not a whole raw track.
    fs::write(dir.join("Short.bin"), vec![0u8; SECTOR * 3 + 100]).unwrap();
    let short = write_cue(
        &dir,
        "Short.cue",
        "FILE \"Short.bin\" BINARY\n  TRACK 01 MODE2/2352\n    INDEX 01 00:00:00\n",
    );
    assert!(inspect_ps1_cue_disc(&short).is_err());
}
