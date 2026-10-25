use super::*;
use crate::wiiu_content_readiness::{WiiUKeyStoreEvidence, project_wiiu_content_readiness};

#[test]
fn grammar_reports_only_counts_or_first_bad_line() {
    // Deliberate zero placeholders, never real keys.
    let zero = "0".repeat(32);
    assert_eq!(
        syntax(b" # private comment\r\n; nothing\n"),
        StoreState::Empty
    );
    assert_eq!(
        syntax(
            format!(
                "{zero} # private comment\r\n{}; label\n",
                "00-00_00\t00 ".repeat(4)
            )
            .as_bytes()
        ),
        StoreState::WellFormedUnverified { entries: 2 }
    );
    for bad in [
        "1".repeat(31),
        "1".repeat(33),
        "g".repeat(32),
        "\u{feff}".into(),
        "\0".into(),
    ] {
        assert_eq!(
            syntax(format!("{zero}\n{bad}\n").as_bytes()),
            StoreState::Malformed { line: 2 }
        );
    }
    assert_eq!(
        syntax(&vec![b' '; MAX_LINE_BYTES + 1]),
        StoreState::LineTooLong { line: 1 }
    );
    let result = format!(
        "{:?}",
        syntax(format!("{zero} # private comment").as_bytes())
    );
    assert!(!result.contains(&zero));
    assert!(!result.contains("private comment"));
}

#[test]
fn line_budget_is_unknown_rather_than_a_claim_of_malformed_syntax() {
    let mut input = vec![b'0'; 32];
    input.extend_from_slice(b" # ");
    input.extend(vec![b'x'; MAX_LINE_BYTES]);
    assert!(!matches!(syntax(&input), StoreState::Malformed { .. }));
    assert_eq!(syntax(&input), StoreState::LineTooLong { line: 1 });
}

#[test]
fn bounded_store_checks_preserve_source_and_do_not_create_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keys.txt");
    assert_eq!(inspect_store(&path).state, StoreState::Missing);
    assert!(!path.exists());
    assert_eq!(
        inspect_store(&dir.path().join("absent/keys.txt")).state,
        StoreState::Missing
    );
    for (bytes, expected) in [
        (Vec::new(), StoreState::Empty),
        (
            vec![b'0'; 32],
            StoreState::WellFormedUnverified { entries: 1 },
        ),
        (b"bad\n".to_vec(), StoreState::Malformed { line: 1 }),
        (vec![b' '; MAX_STORE_BYTES + 1], StoreState::TooLarge),
    ] {
        fs::write(&path, &bytes).unwrap();
        let before = fs::metadata(&path).unwrap();
        let result = inspect_store(&path);
        assert_eq!(result.state, expected);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(
            fingerprint(&before),
            fingerprint(&fs::metadata(&path).unwrap())
        );
    }
    assert_eq!(
        inspect_store(Path::new("keys.txt")).state,
        StoreState::UnsafePath
    );
    assert_eq!(
        inspect_store(&dir.path().join("other.txt")).state,
        StoreState::UnsafePath
    );
    assert_eq!(
        inspect_store(&dir.path().join("../keys.txt")).state,
        StoreState::UnsafePath
    );
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(inspect_store(&path).state, StoreState::UnsafePath);
}

#[cfg(unix)]
#[test]
fn links_and_special_files_are_refused_before_reading() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    fs::write(&target, vec![b'0'; 32]).unwrap();
    let path = dir.path().join("keys.txt");
    symlink(&target, &path).unwrap();
    assert_eq!(inspect_store(&path).state, StoreState::UnsafePath);
    fs::remove_file(&path).unwrap();
    symlink(dir.path().join("absent"), &path).unwrap();
    assert_eq!(inspect_store(&path).state, StoreState::UnsafePath);
    fs::remove_file(&path).unwrap();
    fs::hard_link(&target, &path).unwrap();
    assert_eq!(inspect_store(&path).state, StoreState::UnsafePath);
    fs::remove_file(&path).unwrap();
    let real = dir.path().join("real");
    fs::create_dir(&real).unwrap();
    symlink(&real, dir.path().join("alias")).unwrap();
    assert_eq!(
        inspect_store(&dir.path().join("alias/keys.txt")).state,
        StoreState::UnsafePath
    );
    use std::os::unix::ffi::OsStrExt;
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    assert_eq!(inspect_store(&path).state, StoreState::UnsafePath);
}

#[test]
fn candidate_checks_are_exact_bounded_and_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("Cemu");
    let one = inspect_candidates("profile", dir.path(), Some(&exe));
    assert_eq!(one.locations.len(), 1);
    let two = inspect_candidates(
        "profile",
        dir.path(),
        Some(&dir.path().join("portable/Cemu")),
    );
    assert_eq!(two.locations.len(), 2);
    assert!(two.locations.iter().all(|p| p.state == StoreState::Missing));
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn structural_requirement_never_uses_filename_or_store_presence_as_proof() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("game.wud");
    fs::write(&path, b"not a disc").unwrap();
    let inspect = || project_wiiu_content_readiness(&path, WiiUKeyStoreEvidence::NotChecked, None);
    assert_eq!(requirement(&inspect()), Requirement::Unknown);
    let mut header = vec![0; crate::wiiu_disc::WUD_HEADER_SIZE as usize];
    header[0x10000..0x10004].copy_from_slice(&0xcc54_9eb9_u32.to_be_bytes());
    fs::write(&path, header).unwrap();
    let before = inspect();
    assert_eq!(requirement(&before), Requirement::DiscKeyRequired);
    fs::write(dir.path().join("keys.txt"), vec![b'0'; 32]).unwrap();
    assert_eq!(
        inspect_store(&dir.path().join("keys.txt")).state,
        StoreState::WellFormedUnverified { entries: 1 }
    );
    let after = inspect();
    assert_eq!(format!("{before:?}"), format!("{after:?}"));
    assert_ne!(
        after.key_readiness,
        super::super::WiiUKeyReadiness::AvailableLocally
    );
    // The diagnostic classification also keeps distinct media requirements.
    let mut projection = after;
    projection.representation = Form::Nus;
    assert_eq!(
        requirement(&projection),
        Requirement::EncryptedTitleUnsupported
    );
    projection.representation = Form::Wux;
    assert_eq!(requirement(&projection), Requirement::DiscKeyRequired);
    projection.representation = Form::Wua;
    assert_eq!(requirement(&projection), Requirement::Unknown);
    projection.representation = Form::LoadiineExtracted;
    projection.encryption_state = Encryption::Decrypted;
    assert_eq!(requirement(&projection), Requirement::NotRequired);
    projection.structural_complete = false;
    assert_eq!(requirement(&projection), Requirement::Unknown);
}
