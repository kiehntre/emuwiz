use super::*;
use sha2::{Digest, Sha256};

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn captured_catalogue_bytes_and_digest_survive_source_replacement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalogue.dat");
    let original = b"<datafile><header><name>Original</name></header></datafile>";
    std::fs::write(&path, original).unwrap();
    let (snapshot, sha) = snapshot_dat_file(&path, DatLimits::default()).unwrap();
    std::fs::write(
        &path,
        b"<datafile><header><name>Replacement</name></header></datafile>",
    )
    .unwrap();
    assert_eq!(sha, digest(original));
    assert_eq!(
        parse_dat_file(snapshot.path(), DatLimits::default())
            .unwrap()
            .dat
            .source
            .name
            .as_deref(),
        Some("Original")
    );
    let (parsed, new_sha) = parse_dat_file_bound(&path, DatLimits::default()).unwrap();
    assert_eq!(parsed.dat.source.name.as_deref(), Some("Replacement"));
    assert_eq!(parsed.dat.source.file_path, path.to_string_lossy());
    assert_ne!(sha, new_sha);
}

#[test]
fn a_bound_catalogue_read_enforces_the_size_limit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalogue.dat");
    std::fs::write(&path, [b'x'; 100]).unwrap();
    assert!(matches!(
        parse_dat_file_bound(
            &path,
            DatLimits {
                max_file_size: 50,
                ..Default::default()
            }
        ),
        Err(ParseError::FileTooLarge { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_fifo_substituted_for_a_catalogue_is_refused_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalogue.dat");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: c_path is a live NUL-terminated pathname in this test's tempdir.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    assert!(parse_dat_file_bound(&path, DatLimits::default()).is_err());
}
