use super::*;
use crate::dat::parsers::parse_dat_file;
use sha1::{Digest, Sha1};
use std::fs;
use std::os::unix::fs::PermissionsExt;

const ROM: &[u8] = b"genuine game rom bytes";

fn sha1_hex(bytes: &[u8]) -> String {
    Sha1::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A Logiqx DAT with one entry per `(game, rom name, bytes)`.
fn dat(dir: &Path, name: &str, games: &[(&str, &str, &[u8])]) -> DatIndex {
    let mut xml = format!(
        "<?xml version=\"1.0\"?>\n<datafile>\n<header><name>{name}</name><description>{name}</description></header>\n"
    );
    for (game, rom, bytes) in games {
        xml.push_str(&format!(
            "<game name=\"{game}\"><rom name=\"{rom}\" size=\"{}\" sha1=\"{}\"/></game>\n",
            bytes.len(),
            sha1_hex(bytes)
        ));
    }
    xml.push_str("</datafile>\n");
    let path = dir.join(format!("{name}.dat"));
    fs::write(&path, xml).unwrap();
    DatIndex::build(&parse_dat_file(&path, Default::default()).unwrap().dat)
}

fn library() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    fs::create_dir(&root).unwrap();
    (dir, root)
}

fn check(root: &Path, path: &Path, catalogues: &[CatalogueToCheck<'_>]) -> SupportContentCheck {
    let size = fs::metadata(path).map_or(0, |m| m.len());
    check_support_file_content(
        &[root.to_path_buf()],
        path,
        size,
        catalogues,
        &AtomicBool::new(false),
    )
}

fn catalogue<'a>(label: &str, index: &'a DatIndex) -> CatalogueToCheck<'a> {
    CatalogueToCheck {
        label: label.into(),
        provenance: "User-registered".into(),
        index,
    }
}

#[test]
fn game_content_under_txt_jpg_and_pdf_names_is_an_exact_match() {
    let (dir, root) = library();
    let index = dat(
        dir.path(),
        "Game Boy",
        &[("Tetris (World)", "Tetris.gb", ROM)],
    );
    for name in ["Tetris.txt", "Tetris.jpg", "Tetris.pdf"] {
        let path = root.join(name);
        fs::write(&path, ROM).unwrap();
        let result = check(&root, &path, &[catalogue("Game Boy", &index)]);
        assert_eq!(result.outcome, SupportCheckOutcome::ExactMatch, "{name}");
        assert_eq!(result.checked, vec!["Game Boy".to_string()]);
        let [found] = result.matches.as_slice() else {
            panic!("{result:?}")
        };
        assert_eq!(found.catalogue, "Game Boy");
        assert_eq!(found.provenance, "User-registered");
        assert!(matches!(
            &found.verdict,
            AuditVerdict::Exact { game_name, .. } if game_name == "Tetris (World)"
        ));
        assert!(
            result
                .hashes
                .iter()
                .any(|h| h.algorithm == StrongHashAlgorithm::Sha1 && h.hex == sha1_hex(ROM))
        );
    }
}

#[test]
fn real_artwork_and_manuals_report_no_match_in_the_checked_catalogues() {
    let (dir, root) = library();
    let a = dat(
        dir.path(),
        "Game Boy",
        &[("Tetris (World)", "Tetris.gb", ROM)],
    );
    let b = dat(dir.path(), "Mega Drive", &[("Sonic", "Sonic.md", b"sonic")]);
    for (name, bytes) in [
        ("cover.png", &b"\x89PNG art"[..]),
        ("manual.pdf", b"%PDF-1.4"),
    ] {
        let path = root.join(name);
        fs::write(&path, bytes).unwrap();
        let result = check(
            &root,
            &path,
            &[catalogue("Game Boy", &a), catalogue("Mega Drive", &b)],
        );
        assert_eq!(
            result.outcome,
            SupportCheckOutcome::NoMatchInCheckedCatalogues
        );
        assert!(result.matches.is_empty());
        assert_eq!(
            result.checked,
            vec!["Game Boy".to_string(), "Mega Drive".to_string()]
        );
        assert!(!result.hashes.is_empty(), "the computed checksum is shown");
    }
}

#[test]
fn without_an_installed_catalogue_nothing_is_compared_or_concluded() {
    let (_dir, root) = library();
    let path = root.join("Tetris.jpg");
    fs::write(&path, ROM).unwrap();
    let result = check(&root, &path, &[]);
    assert_eq!(result.outcome, SupportCheckOutcome::NoCatalogueToCompare);
    assert!(result.checked.is_empty() && result.matches.is_empty());
}

#[test]
fn content_in_several_entries_or_catalogues_is_ambiguous() {
    let (dir, root) = library();
    let path = root.join("dump.txt");
    fs::write(&path, ROM).unwrap();
    // Two entries in one catalogue.
    let twice = dat(
        dir.path(),
        "Game Boy",
        &[
            ("Tetris (World)", "a.gb", ROM),
            ("Tetris (Rev 1)", "b.gb", ROM),
        ],
    );
    let result = check(&root, &path, &[catalogue("Game Boy", &twice)]);
    assert_eq!(result.outcome, SupportCheckOutcome::AmbiguousMatch);
    // One entry in each of two catalogues.
    let a = dat(dir.path(), "A", &[("Tetris", "a.gb", ROM)]);
    let b = dat(dir.path(), "B", &[("Tetris", "b.gb", ROM)]);
    let result = check(&root, &path, &[catalogue("A", &a), catalogue("B", &b)]);
    assert_eq!(result.outcome, SupportCheckOutcome::AmbiguousMatch);
    assert_eq!(result.matches.len(), 2);
}

#[test]
fn a_file_replaced_after_listing_is_not_checked() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let path = root.join("Tetris.jpg");
    fs::write(&path, b"art").unwrap();
    let listed = fs::metadata(&path).unwrap().len();
    // Replaced by the ROM bytes after Library Files Check listed it.
    fs::write(&path, ROM).unwrap();
    let result = check_support_file_content(
        &[root.clone()],
        &path,
        listed,
        &[catalogue("Game Boy", &index)],
        &AtomicBool::new(false),
    );
    assert_eq!(result.outcome, SupportCheckOutcome::ChangedSinceListed);
    assert!(result.hashes.is_empty() && result.matches.is_empty());
}

#[test]
fn unreadable_missing_and_escaping_files_are_reported_unreadable() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let locked = root.join("locked.pdf");
    fs::write(&locked, ROM).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let missing = root.join("gone.png");
    // A symlink inside the root pointing outside it is refused by the pinned root.
    let outside = dir.path().join("outside.bin");
    fs::write(&outside, ROM).unwrap();
    let link = root.join("link.jpg");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    for path in [&locked, &missing, &link] {
        let result = check_support_file_content(
            &[root.clone()],
            path,
            ROM.len() as u64,
            &[catalogue("Game Boy", &index)],
            &AtomicBool::new(false),
        );
        if path == &locked && fs::File::open(&locked).is_ok() {
            continue; // running with CAP_DAC_OVERRIDE: permissions do not apply
        }
        assert_eq!(result.outcome, SupportCheckOutcome::Unreadable, "{path:?}");
        assert!(result.hashes.is_empty() && result.matches.is_empty());
    }
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn only_the_selected_file_is_read() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let selected = root.join("Tetris.jpg");
    fs::write(&selected, ROM).unwrap();
    for i in 0..20 {
        fs::write(root.join(format!("cover{i}.png")), vec![b'x'; 1000]).unwrap();
    }
    let result = check(&root, &selected, &[catalogue("Game Boy", &index)]);
    assert_eq!(result.outcome, SupportCheckOutcome::ExactMatch);
    // Exactly the selected file's bytes were read; 20 KB of artwork was not.
    assert_eq!(result.bytes_read, ROM.len() as u64);
    assert_eq!(result.path, selected);
}

#[test]
fn a_cancelled_check_reports_cancelled() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let path = root.join("Tetris.jpg");
    fs::write(&path, ROM).unwrap();
    let result = check_support_file_content(
        &[root.clone()],
        &path,
        ROM.len() as u64,
        &[catalogue("Game Boy", &index)],
        &AtomicBool::new(true),
    );
    assert_eq!(result.outcome, SupportCheckOutcome::Cancelled);
}
