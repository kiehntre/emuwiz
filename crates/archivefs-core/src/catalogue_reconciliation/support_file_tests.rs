//! Physical-file classification: artwork, manuals and other support files stay
//! in the physical total but are never uncatalogued games, move targets or
//! hash requests. Unknown files are reported on their own, never as support.
use super::walk::{RootWalkState, WalkLimits, WalkRoot, walk_roots};
use super::*;
use std::fs;
use std::sync::atomic::AtomicBool;

fn h1(n: u8) -> StrongHash {
    StrongHash::new(StrongHashAlgorithm::Sha1, &format!("{n:02x}").repeat(20)).unwrap()
}
fn row(id: i64, path: &str, size: u64, presence: RowPresence, hashes: Vec<StrongHash>) -> RowFacts {
    RowFacts {
        archive_id: id,
        source_id: 1,
        path: path.into(),
        size: Some(size),
        platform: Some("GBA".into()),
        presence,
        hashes,
    }
}
fn file(path: &str, size: u64, hashes: Vec<StrongHash>) -> FileFacts {
    FileFacts {
        path: path.into(),
        source_id: 1,
        size,
        hashes,
    }
}
fn outcome<'a>(report: &'a ReconciliationReport, path: &str) -> &'a FileOutcome {
    report
        .files
        .iter()
        .find(|f| f.path == Path::new(path))
        .unwrap()
}
fn invariant(report: &ReconciliationReport) {
    let c = &report.counts;
    assert_eq!(
        c.files,
        c.catalogued_files
            + c.uncatalogued_files
            + c.known_support_files
            + c.support_review_files
            + c.unrecognised_files
            + c.referenced_companions
            + c.contended_companions,
        "{c:?}"
    );
    assert_eq!(c.files, report.files.len());
}

#[test]
fn artwork_and_manuals_are_support_counted_but_never_uncatalogued_games() {
    for name in [
        "cover.png",
        "cover.jpg",
        "cover.jpeg",
        "manual.pdf",
        "COVER.PNG",
    ] {
        let path = format!("/lib/{name}");
        let report = reconcile(&[], &[file(&path, 4, vec![])]);
        let out = outcome(&report, &path);
        assert_eq!(out.class, PhysicalFileClass::KnownSupport, "{name}");
        assert_eq!(out.state, FileState::KnownSupportFile, "{name}");
        assert_eq!(report.counts.files, 1, "physical total keeps it");
        assert_eq!(report.counts.known_support_files, 1);
        assert_eq!(report.counts.uncatalogued_files, 0);
        assert_eq!(report.counts.unrecognised_files, 0);
        assert!(report.hash_requests.is_empty());
        invariant(&report);
    }
}

#[test]
fn support_files_are_never_move_candidates_or_hash_requests() {
    // Same size, same name stem, and a stored strong hash: only a game file
    // may ever be offered.
    let rows = [row(
        1,
        "/lib/game.gba",
        4,
        RowPresence::Missing,
        vec![h1(7)],
    )];
    for support in [
        "cover.jpg",
        "cover.png",
        "manual.pdf",
        "game.pdf",
        "game.gba.png",
    ] {
        let path = format!("/lib/{support}");
        // A support file is never offered, even one that already carries the
        // row's checksum. That one is listed for review instead of being set
        // aside, because the checksum says it may be the game itself.
        for hashes in [vec![], vec![h1(7)]] {
            let matching = !hashes.is_empty();
            let report = reconcile(&rows, &[file(&path, 4, hashes)]);
            assert_eq!(
                report.rows[0].state,
                RowState::CatalogueFileMissing,
                "{support}"
            );
            assert!(report.hash_requests.is_empty(), "{support}");
            let expected = if matching {
                FileState::SupportNeedsReview {
                    evidence: SupportContradiction::ChecksumMatchesCatalogueEntry { archive_id: 1 },
                }
            } else {
                FileState::KnownSupportFile
            };
            assert_eq!(outcome(&report, &path).state, expected, "{support}");
            invariant(&report);
        }
    }
    // Control: a same-size game file still is a candidate and is hash-requested.
    let report = reconcile(&rows, &[file("/lib/moved.gba", 4, vec![])]);
    assert!(matches!(
        report.rows[0].state,
        RowState::PossiblyMoved {
            proof: MoveProof::NeedsHashing,
            ..
        }
    ));
    assert_eq!(report.hash_requests, vec![PathBuf::from("/lib/moved.gba")]);
}

#[test]
fn a_support_file_with_the_rows_name_and_size_is_not_possibly_moved() {
    // A row without a stored hash: name+size is the only weak basis.
    let report = reconcile(
        &[row(1, "/lib/manual.pdf", 4, RowPresence::Missing, vec![])],
        &[file("/lib/elsewhere/manual.pdf", 4, vec![])],
    );
    assert_eq!(report.rows[0].state, RowState::CatalogueFileMissing);
    assert!(report.hash_requests.is_empty());
    // The catalogue's own name and size contradict the extension: reviewed,
    // not silently set aside.
    assert_eq!(
        outcome(&report, "/lib/elsewhere/manual.pdf").state,
        FileState::SupportNeedsReview {
            evidence: SupportContradiction::NameAndSizeMatchCatalogueEntry { archive_id: 1 },
        }
    );
    assert_eq!(report.counts.support_review_files, 1);
    assert_eq!(report.counts.known_support_files, 0);
    invariant(&report);
}

#[test]
fn the_mega_drive_md_extension_is_never_hidden_as_support() {
    assert_eq!(
        classify_physical_file(Path::new("/lib/Sonic.md")),
        PhysicalFileClass::GameOrMedia
    );
    let report = reconcile(&[], &[file("/lib/Sonic.md", 4, vec![])]);
    assert_eq!(
        outcome(&report, "/lib/Sonic.md").state,
        FileState::Uncatalogued
    );
    assert_eq!(report.counts.uncatalogued_files, 1);
    assert_eq!(report.counts.known_support_files, 0);
}

#[test]
fn recognised_game_and_archive_files_remain_uncatalogued_candidates() {
    for name in [
        "Game.gba", "Game.zip", "Game.7z", "Game.iso", "Game.cue", "Game.bin",
    ] {
        let path = format!("/lib/{name}");
        let report = reconcile(&[], &[file(&path, 4, vec![])]);
        assert_eq!(
            outcome(&report, &path).state,
            FileState::Uncatalogued,
            "{name}"
        );
        assert_eq!(report.counts.uncatalogued_files, 1, "{name}");
    }
}

#[test]
fn an_unknown_extension_is_reported_separately_and_not_as_support() {
    for name in [
        "thing.qzx9",
        "noextension",
        "archive.cbz",
        "book.epub",
        "disc.7z.001",
    ] {
        let path = format!("/lib/{name}");
        assert_eq!(
            classify_physical_file(Path::new(&path)),
            PhysicalFileClass::Unrecognised,
            "{name}"
        );
        let report = reconcile(&[], &[file(&path, 4, vec![])]);
        assert_eq!(
            outcome(&report, &path).state,
            FileState::UnrecognisedFile,
            "{name}"
        );
        assert_eq!(report.counts.unrecognised_files, 1);
        assert_eq!(report.counts.known_support_files, 0);
        assert_eq!(report.counts.uncatalogued_files, 0);
        invariant(&report);
    }
}

#[test]
fn dotfile_housekeeping_uses_the_shared_support_list() {
    assert_eq!(
        classify_physical_file(Path::new("/lib/.DS_Store")),
        PhysicalFileClass::KnownSupport
    );
    assert_eq!(
        classify_physical_file(Path::new("/lib/Thumbs.db")),
        PhysicalFileClass::KnownSupport
    );
}

#[test]
fn an_unknown_file_may_only_replace_a_row_with_the_same_extension() {
    let row_missing = |path: &str| [row(1, path, 4, RowPresence::Missing, vec![h1(3)])];
    // Same recorded extension (an unsupported format catalogued before): a
    // size-matching unknown file is still a weak candidate needing a hash.
    let report = reconcile(
        &row_missing("/lib/old/disc.qzx9"),
        &[file("/lib/new/other.qzx9", 4, vec![])],
    );
    assert!(matches!(
        report.rows[0].state,
        RowState::PossiblyMoved { .. }
    ));
    assert_eq!(
        report.hash_requests,
        vec![PathBuf::from("/lib/new/other.qzx9")]
    );
    // A different extension is never offered, whatever the size.
    let report = reconcile(
        &row_missing("/lib/old/game.gba"),
        &[file("/lib/new/other.qzx9", 4, vec![])],
    );
    assert_eq!(report.rows[0].state, RowState::CatalogueFileMissing);
    assert!(report.hash_requests.is_empty());
    // A strong hash match on an unknown file of another extension is not offered.
    let report = reconcile(
        &row_missing("/lib/old/game.gba"),
        &[file("/lib/new/other.qzx9", 4, vec![h1(3)])],
    );
    assert_eq!(report.rows[0].state, RowState::CatalogueFileMissing);
}

#[test]
fn a_catalogued_path_stays_catalogued_beside_support_files() {
    let report = reconcile(
        &[row(1, "/lib/Game.gba", 4, RowPresence::Present, vec![])],
        &[
            file("/lib/Game.gba", 4, vec![]),
            file("/lib/Game.png", 4, vec![]),
            file("/lib/Game.pdf", 4, vec![]),
        ],
    );
    assert_eq!(
        outcome(&report, "/lib/Game.gba").state,
        FileState::Catalogued { archive_id: 1 }
    );
    assert_eq!(report.counts.catalogued_files, 1);
    assert_eq!(report.counts.known_support_files, 2);
    // Even a catalogued row whose own extension is a support type stays catalogued.
    let report = reconcile(
        &[row(2, "/lib/odd.png", 4, RowPresence::Present, vec![])],
        &[file("/lib/odd.png", 4, vec![])],
    );
    assert_eq!(
        outcome(&report, "/lib/odd.png").state,
        FileState::Catalogued { archive_id: 2 }
    );
    invariant(&report);
}

#[test]
fn companion_ownership_wins_over_generic_classification() {
    let links = [CompanionLink {
        parent_archive_id: 1,
        basis: CompanionBasis::CueFileReference,
        files: vec!["/lib/track01.png".into(), "/lib/track02.bin".into()],
    }];
    let report = reconcile_with_companions(
        &[row(1, "/lib/Game.cue", 1, RowPresence::Present, vec![])],
        &[
            file("/lib/Game.cue", 1, vec![]),
            file("/lib/track01.png", 4, vec![]),
            file("/lib/track02.bin", 4, vec![]),
        ],
        &links,
    );
    for path in ["/lib/track01.png", "/lib/track02.bin"] {
        assert!(
            matches!(
                outcome(&report, path).state,
                FileState::ReferencedCompanion { .. }
            ),
            "{path}"
        );
    }
    assert_eq!(report.counts.referenced_companions, 2);
    assert_eq!(report.counts.known_support_files, 0);
    invariant(&report);
}

#[test]
fn a_mixed_library_keeps_the_physical_total_but_counts_three_games() {
    let mut files = vec![
        file("/lib/a.gba", 10, vec![]),
        file("/lib/b.gba", 11, vec![]),
        file("/lib/c.zip", 12, vec![]),
    ];
    for i in 0..60 {
        files.push(file(&format!("/lib/media/{i}.png"), 4, vec![]));
    }
    for i in 0..30 {
        files.push(file(&format!("/lib/media/{i}.jpg"), 4, vec![]));
    }
    for i in 0..10 {
        files.push(file(&format!("/lib/manuals/{i}.pdf"), 4, vec![]));
    }
    let report = reconcile(&[], &files);
    assert_eq!(report.counts.files, 103);
    assert_eq!(report.counts.uncatalogued_files, 3);
    assert_eq!(report.counts.known_support_files, 100);
    assert_eq!(report.counts.unrecognised_files, 0);
    invariant(&report);
}

#[test]
fn every_registry_extension_is_game_or_media_and_pdf_collides_with_none() {
    for format in crate::ingestion::content_registry::recognized_extensions() {
        assert!(
            !is_known_non_game_extension(format),
            "content registry extension {format} is also a support extension"
        );
    }
    for format in media_registry::MEDIA_FORMATS {
        assert!(
            !is_known_non_game_extension(format.extension),
            "media registry extension {} is also a support extension",
            format.extension
        );
    }
    for candidate in ["md", "bin", "gen"] {
        assert!(media_registry::is_corroboration_candidate(candidate));
        assert!(!is_known_non_game_extension(candidate));
    }
    assert!(is_known_non_game_extension("pdf"));
    assert!(
        crate::ingestion::content_registry::content_kind_for_extension("pdf").is_none()
            && !media_registry::is_watch_relevant_extension("pdf")
    );
    for deliberately_visible in ["cbz", "cbr", "epub"] {
        assert!(!is_known_non_game_extension(deliberately_visible));
    }
}

#[test]
fn a_partial_walk_stays_partial_and_classification_adds_no_coverage() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("gba");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("game.gba"), b"gggg").unwrap();
    fs::write(root.join("cover.png"), b"pppp").unwrap();
    fs::write(root.join("sub/hidden.gba"), b"hhhh").unwrap();
    fs::write(root.join("sub/hidden.png"), b"hhhh").unwrap();
    let nested = root.join("sub");
    let walked = walk_roots(
        &[WalkRoot {
            source_id: 1,
            path: root.clone(),
            excluded: vec![],
        }],
        WalkLimits::default(),
        &AtomicBool::new(false),
        &|path: &Path| Ok(if path.starts_with(&nested) { 2 } else { 1 }),
    );
    assert_eq!(walked.roots[0].state, RootWalkState::Partial);
    let report = reconcile(&[], &walked.files);
    // Only what was walked is counted; the unexplored folder is not invented.
    assert_eq!(report.counts.files, 2);
    assert_eq!(report.counts.uncatalogued_files, 1);
    assert_eq!(report.counts.known_support_files, 1);
    assert_eq!(walked.roots[0].nested_boundaries, vec![nested]);
}

#[test]
fn unusual_media_and_misleading_names_follow_the_final_extension_only() {
    // Uncommon but registered game media stays a game/media candidate.
    for name in [
        "Jet Set Willy.tzx",
        "Elite.d64",
        "Lemmings.adf",
        "Dungeon Master.st",
        "Turrican.ipf",
        "Wario Land.vb",
        "Disc.chd",
        "Sonic.md",
        // A support word in the name, but a game/container extension.
        "manual.pdf.zip",
        "cover.jpg.7z",
        "Screenshot.nds",
    ] {
        let path = format!("/lib/{name}");
        let report = reconcile(&[], &[file(&path, 4, vec![])]);
        let out = outcome(&report, &path);
        assert_eq!(out.class, PhysicalFileClass::GameOrMedia, "{name}");
        assert_eq!(out.state, FileState::Uncatalogued, "{name}");
        assert_eq!(report.counts.uncatalogued_files, 1, "{name}");
        invariant(&report);
    }
    // Screenshots and box art under any name are support, including a game
    // name with an image extension; an extensionless file is unrecognised,
    // never support.
    for (name, class) in [
        ("snap/Sonic (USA).png", PhysicalFileClass::KnownSupport),
        ("boxart/Elite.gif", PhysicalFileClass::KnownSupport),
        ("Elite.d64.webp", PhysicalFileClass::KnownSupport),
        ("README", PhysicalFileClass::Unrecognised),
        ("Game.unknownext", PhysicalFileClass::Unrecognised),
    ] {
        let path = format!("/lib/{name}");
        let report = reconcile(&[], &[file(&path, 4, vec![])]);
        assert_eq!(outcome(&report, &path).class, class, "{name}");
        assert_eq!(report.counts.uncatalogued_files, 0, "{name}");
        invariant(&report);
    }
}

#[test]
fn a_rom_saved_under_an_image_or_pdf_name_is_reviewed_not_set_aside_or_offered() {
    // A present game and a missing one, both with stored checksums. Copies
    // of their bytes under support-file names must neither vanish into the
    // support count nor become move targets or hash requests.
    let rows = [
        row(1, "/lib/present.gba", 4, RowPresence::Present, vec![h1(1)]),
        row(2, "/lib/missing.gba", 4, RowPresence::Missing, vec![h1(2)]),
    ];
    let files = [
        file("/lib/present.gba", 4, vec![h1(1)]),
        file("/lib/copy-of-present.jpg", 4, vec![h1(1)]),
        file("/lib/missing-renamed.pdf", 4, vec![h1(2)]),
        file("/lib/cover.png", 4, vec![h1(9)]),
    ];
    let report = reconcile(&rows, &files);
    assert_eq!(report.rows[1].state, RowState::CatalogueFileMissing);
    assert!(report.hash_requests.is_empty());
    assert_eq!(
        outcome(&report, "/lib/copy-of-present.jpg").state,
        FileState::SupportNeedsReview {
            evidence: SupportContradiction::ChecksumMatchesCatalogueEntry { archive_id: 1 },
        }
    );
    assert_eq!(
        outcome(&report, "/lib/missing-renamed.pdf").state,
        FileState::SupportNeedsReview {
            evidence: SupportContradiction::ChecksumMatchesCatalogueEntry { archive_id: 2 },
        }
    );
    // An unrelated checksum is no evidence: still set aside.
    assert_eq!(
        outcome(&report, "/lib/cover.png").state,
        FileState::KnownSupportFile
    );
    assert_eq!(report.counts.support_review_files, 2);
    assert_eq!(report.counts.known_support_files, 1);
    assert_eq!(report.counts.catalogued_files, 1);
    assert_eq!(report.counts.uncatalogued_files, 0);
    invariant(&report);
}

#[test]
fn support_categories_label_every_shared_support_extension() {
    for (name, category) in [
        ("a.jpg", SupportCategory::Artwork),
        ("a.JPEG", SupportCategory::Artwork),
        ("a.png", SupportCategory::Artwork),
        ("a.gif", SupportCategory::Artwork),
        ("a.bmp", SupportCategory::Artwork),
        ("a.webp", SupportCategory::Artwork),
        ("a.tga", SupportCategory::Artwork),
        ("a.ico", SupportCategory::Artwork),
        ("a.pdf", SupportCategory::Manual),
        ("a.txt", SupportCategory::TextOrMetadata),
        ("a.nfo", SupportCategory::TextOrMetadata),
        ("a.xml", SupportCategory::TextOrMetadata),
        ("a.json", SupportCategory::TextOrMetadata),
        ("a.yml", SupportCategory::TextOrMetadata),
        ("a.yaml", SupportCategory::TextOrMetadata),
        ("a.ini", SupportCategory::TextOrMetadata),
        ("a.cfg", SupportCategory::TextOrMetadata),
        ("a.conf", SupportCategory::TextOrMetadata),
        ("a.log", SupportCategory::TextOrMetadata),
        ("a.db", SupportCategory::Housekeeping),
        ("a.url", SupportCategory::Housekeeping),
        ("a.lnk", SupportCategory::Housekeeping),
        (".DS_Store", SupportCategory::Housekeeping),
        ("a.mp4", SupportCategory::VideoOrAudio),
        ("a.webm", SupportCategory::VideoOrAudio),
        ("a.mp3", SupportCategory::VideoOrAudio),
        ("a.ogg", SupportCategory::VideoOrAudio),
    ] {
        let path = Path::new(name);
        // Only files the shared list really sets aside are labelled.
        assert_eq!(
            classify_physical_file(path),
            PhysicalFileClass::KnownSupport,
            "{name}"
        );
        assert_eq!(support_category(path), category, "{name}");
    }
}

#[test]
fn ten_thousand_support_files_keep_exact_counts_and_no_requests() {
    let rows = [row(
        1,
        "/lib/game.gba",
        4,
        RowPresence::Missing,
        vec![h1(7)],
    )];
    let mut files: Vec<FileFacts> = (0..10_000)
        .map(|i| file(&format!("/lib/art/{i:05}.png"), 4, vec![]))
        .collect();
    files.push(file("/lib/moved.gba", 4, vec![]));
    let report = reconcile(&rows, &files);
    assert_eq!(report.counts.known_support_files, 10_000);
    assert_eq!(report.counts.uncatalogued_files, 1);
    assert_eq!(report.hash_requests, vec![PathBuf::from("/lib/moved.gba")]);
    invariant(&report);
}
