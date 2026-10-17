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

/// The identity a Library Files Check walk would record now.
fn listed(path: &Path) -> FileIdentity {
    FileIdentity::from_metadata(&fs::symlink_metadata(path).unwrap())
}

fn check_listed(
    root: &Path,
    path: &Path,
    listed: &FileIdentity,
    catalogues: &[CatalogueToCheck<'_>],
) -> SupportContentCheck {
    check_support_file_content(
        &[root.to_path_buf()],
        path,
        listed,
        catalogues,
        &AtomicBool::new(false),
    )
}

/// Listed and checked immediately: the unchanged-file case.
fn check(root: &Path, path: &Path, catalogues: &[CatalogueToCheck<'_>]) -> SupportContentCheck {
    check_listed(root, path, &listed(path), catalogues)
}

fn refused(result: &SupportContentCheck) {
    assert_eq!(
        result.outcome,
        SupportCheckOutcome::ChangedSinceListed,
        "{result:?}"
    );
    assert!(result.hashes.is_empty() && result.matches.is_empty());
    assert_eq!(result.bytes_read, 0);
}

fn set_mtime(path: &Path, seconds: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
        .unwrap();
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
fn a_different_file_of_identical_size_is_refused() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let path = root.join("Tetris.jpg");
    fs::write(&path, vec![b'a'; ROM.len()]).unwrap();
    set_mtime(&path, 1_000_000);
    let before = listed(&path);
    // Swapped for the ROM: same size and even the same modification time,
    // but a different file (new inode) renamed into place.
    let new = root.join("incoming.tmp");
    fs::write(&new, ROM).unwrap();
    set_mtime(&new, 1_000_000);
    fs::rename(&new, &path).unwrap();
    assert_eq!(listed(&path).size, before.size);
    assert_eq!(listed(&path).modified, before.modified);
    refused(&check_listed(
        &root,
        &path,
        &before,
        &[catalogue("Game Boy", &index)],
    ));
}

#[test]
fn a_size_changing_replacement_and_a_detectable_in_place_edit_are_refused() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let path = root.join("Tetris.jpg");
    fs::write(&path, b"art").unwrap();
    let before = listed(&path);
    fs::write(&path, ROM).unwrap();
    refused(&check_listed(
        &root,
        &path,
        &before,
        &[catalogue("Game Boy", &index)],
    ));
    // Same inode and size, content rewritten in place: the new modification
    // time shows it is no longer what was listed.
    fs::write(&path, vec![b'a'; ROM.len()]).unwrap();
    set_mtime(&path, 1_000_000);
    let before = listed(&path);
    fs::write(&path, ROM).unwrap();
    set_mtime(&path, 2_000_000);
    assert_eq!(listed(&path).inode, before.inode);
    refused(&check_listed(
        &root,
        &path,
        &before,
        &[catalogue("Game Boy", &index)],
    ));
}

#[test]
fn a_symlink_substituted_for_the_listed_file_is_refused() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let path = root.join("Tetris.jpg");
    fs::write(&path, ROM).unwrap();
    let before = listed(&path);
    let elsewhere = root.join("real.bin");
    fs::rename(&path, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
    refused(&check_listed(
        &root,
        &path,
        &before,
        &[catalogue("Game Boy", &index)],
    ));
}

#[test]
fn a_file_replaced_during_hashing_yields_no_checksum() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let path = root.join("Tetris.jpg");
    fs::write(&path, ROM).unwrap();
    let before = listed(&path);
    let swap = path.clone();
    let incoming = root.join("incoming.tmp");
    fs::write(&incoming, vec![b'z'; ROM.len()]).unwrap();
    super::super::hashing::DURING_READ.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || fs::rename(&incoming, &swap).unwrap()));
    });
    let result = check_listed(&root, &path, &before, &[catalogue("Game Boy", &index)]);
    assert_eq!(
        result.outcome,
        SupportCheckOutcome::ChangedSinceListed,
        "{result:?}"
    );
    assert!(result.hashes.is_empty() && result.matches.is_empty());
}

#[test]
fn deleted_unreadable_and_escaping_files_are_never_checked() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    // Listed, then deleted: the listing is out of date.
    let gone = root.join("gone.png");
    fs::write(&gone, ROM).unwrap();
    let gone_identity = listed(&gone);
    fs::remove_file(&gone).unwrap();
    let result = check_listed(
        &root,
        &gone,
        &gone_identity,
        &[catalogue("Game Boy", &index)],
    );
    assert_eq!(result.outcome, SupportCheckOutcome::ChangedSinceListed);
    assert!(result.hashes.is_empty());
    // Listed but unreadable (skipped when running with CAP_DAC_OVERRIDE).
    let locked = root.join("locked.pdf");
    fs::write(&locked, ROM).unwrap();
    let locked_identity = listed(&locked);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&locked).is_err() {
        let result = check_listed(
            &root,
            &locked,
            &locked_identity,
            &[catalogue("Game Boy", &index)],
        );
        assert_eq!(result.outcome, SupportCheckOutcome::Unreadable);
        assert!(result.hashes.is_empty());
    }
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o600)).unwrap();
    // Outside every configured root: not read.
    let outside = dir.path().join("outside.jpg");
    fs::write(&outside, ROM).unwrap();
    let result = check_listed(
        &root,
        &outside,
        &listed(&outside),
        &[catalogue("Game Boy", &index)],
    );
    assert_eq!(result.outcome, SupportCheckOutcome::Unreadable);
    assert!(result.hashes.is_empty());
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
        &listed(&path),
        &[catalogue("Game Boy", &index)],
        &AtomicBool::new(true),
    );
    assert_eq!(result.outcome, SupportCheckOutcome::Cancelled);
}

#[test]
fn a_same_size_rewrite_that_restores_mtime_is_refused() {
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    let path = root.join("Tetris.jpg");
    fs::write(&path, vec![b'a'; ROM.len()]).unwrap();
    set_mtime(&path, 1_000_000);
    let before = listed(&path);
    // Same inode, same size, mtime put back (as `touch -r` would): only the
    // inode change time shows the content was replaced.
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(&path, ROM).unwrap();
    set_mtime(&path, 1_000_000);
    let now = listed(&path);
    assert_eq!(
        (now.inode, now.size, now.modified, now.modified_ns),
        (
            before.inode,
            before.size,
            before.modified,
            before.modified_ns
        )
    );
    refused(&check_listed(
        &root,
        &path,
        &before,
        &[catalogue("Game Boy", &index)],
    ));
}

#[test]
fn a_real_walk_then_a_check_accepts_the_unchanged_file() {
    use super::super::walk::{WalkLimits, WalkRoot, walk_roots};
    let (dir, root) = library();
    let index = dat(dir.path(), "Game Boy", &[("Tetris", "Tetris.gb", ROM)]);
    fs::write(root.join("Tetris.jpg"), ROM).unwrap();
    fs::write(root.join("cover.png"), b"art").unwrap();
    let walked = walk_roots(
        &[WalkRoot {
            source_id: 1,
            path: root.clone(),
            excluded: vec![],
        }],
        WalkLimits::default(),
        &AtomicBool::new(false),
        &|_: &Path| Ok(1),
    );
    let report = super::super::reconcile(&[], &walked.files);
    let listed_file = report
        .files
        .iter()
        .find(|file| file.path.ends_with("Tetris.jpg"))
        .unwrap();
    let identity = listed_file.identity.expect("the walk records identity");
    let result = check_listed(
        &root,
        &listed_file.path,
        &identity,
        &[catalogue("Game Boy", &index)],
    );
    assert_eq!(result.outcome, SupportCheckOutcome::ExactMatch);
    assert_eq!(result.bytes_read, ROM.len() as u64);
}
