use super::*;
use crate::identity_source::managed_snapshot::SourceResponseMetadata;
use std::cell::RefCell;
use std::io::Write;
use std::net::IpAddr;

const TOKEN: &str = "s3cretT0ken";

fn dat(version: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0"?><datafile><header><name>Custom</name><version>{version}</version><author>User</author></header><game name="Example"><rom name="example.bin" size="1" sha1="86f7e437fa060d3f29a2f2c2e1f7b8f7d3b2e0d4"/></game></datafile>"#
    )
    .into_bytes()
}

fn zip_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in members {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn public() -> StaticResolver {
    StaticResolver::new().with("example.test", &["8.8.8.8".parse::<IpAddr>().unwrap()])
}

#[derive(Default)]
struct Fixture {
    status: u16,
    etag: Option<String>,
    body: Vec<u8>,
    fail_with: Option<String>,
    fetches: RefCell<usize>,
}

impl Fixture {
    fn ok(body: Vec<u8>, etag: &str) -> Self {
        Self {
            status: 200,
            etag: Some(etag.into()),
            body,
            ..Self::default()
        }
    }
}

impl ManagedSourceTransport for Fixture {
    fn metadata(
        &self,
        _url: &str,
        _headers: &[(String, String)],
    ) -> std::result::Result<SourceResponseMetadata, String> {
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        Ok(SourceResponseMetadata {
            status: self.status,
            etag: self.etag.clone(),
            ..SourceResponseMetadata::default()
        })
    }

    fn fetch(
        &self,
        _url: &str,
        _headers: &[(String, String)],
        maximum_size: u64,
        destination: &mut dyn Write,
    ) -> std::result::Result<SourceResponseMetadata, String> {
        *self.fetches.borrow_mut() += 1;
        if let Some(error) = &self.fail_with {
            return Err(error.clone());
        }
        if self.body.len() as u64 > maximum_size {
            return Err("too large".into());
        }
        destination
            .write_all(&self.body)
            .map_err(|error| error.to_string())?;
        Ok(SourceResponseMetadata {
            status: self.status,
            etag: self.etag.clone(),
            content_length: Some(self.body.len() as u64),
            ..SourceResponseMetadata::default()
        })
    }
}

fn source() -> CustomDatSource {
    CustomDatSource::https("https://example.test/catalogue.dat", "Example").unwrap()
}

fn staging_is_empty(root: &Path) -> bool {
    match fs::read_dir(root.join("staging")) {
        Ok(entries) => entries.count() == 0,
        Err(_) => true,
    }
}

// --- registration / policy ----------------------------------------------

#[test]
fn https_and_github_sources_register_without_touching_the_network() {
    let https = source();
    assert_eq!(https.kind, CustomDatSourceKind::Https);
    assert_eq!(https.trust, DatAcquisitionTrust::UserProvided);
    let github = CustomDatSource::github(
        "https://github.com/acme/catalogue/releases/download/v1/all.zip",
        "Acme",
    )
    .unwrap();
    assert_eq!(github.kind, CustomDatSourceKind::Github);
    let acquisition = github.acquisition.unwrap();
    assert_eq!(acquisition.repository.as_deref(), Some("acme/catalogue"));
    assert_eq!(acquisition.release_or_tag.as_deref(), Some("v1"));
}

#[test]
fn http_is_rejected() {
    assert!(CustomDatSource::https("http://example.test/a.dat", "x").is_err());
}

#[test]
fn embedded_credentials_are_rejected_without_echoing_them() {
    let url = format!("https://user:{TOKEN}@example.test/a.dat");
    let error = CustomDatSource::https(&url, "x").unwrap_err().to_string();
    assert!(error.contains("credentials"));
    assert!(!error.contains(TOKEN), "{error}");
}

#[test]
fn loopback_private_and_metadata_literals_are_rejected_at_registration() {
    for url in [
        "https://localhost/a.dat",
        "https://foo.localhost/a.dat",
        "https://127.0.0.1/a.dat",
        "https://10.1.2.3/a.dat",
        "https://192.168.0.5/a.dat",
        "https://169.254.169.254/a.dat",
        "https://[::1]/a.dat",
    ] {
        assert!(CustomDatSource::https(url, "x").is_err(), "{url}");
    }
}

#[test]
fn a_name_that_resolves_to_a_private_address_is_refused_before_any_request() {
    let dir = tempfile::tempdir().unwrap();
    let source = source();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let rebinding =
        StaticResolver::new().with("example.test", &["10.0.0.7".parse::<IpAddr>().unwrap()]);
    let transport = Fixture::ok(dat("1"), "e1");
    let error = lifecycle
        .download_with(&rebinding, &transport)
        .unwrap_err()
        .to_string();
    assert!(error.contains("source refused"), "{error}");
    assert_eq!(*transport.fetches.borrow(), 0, "no request may be made");
    assert!(
        lifecycle
            .check_update_with(false, &rebinding, &transport)
            .is_err()
    );
}

#[test]
fn github_hosting_never_makes_a_source_official() {
    let github = CustomDatSource::github(
        "https://raw.githubusercontent.com/acme/catalogue/main/a.dat",
        "Acme",
    )
    .unwrap();
    assert_eq!(github.trust, DatAcquisitionTrust::UserProvided);
    assert_eq!(github.descriptor().trust, ManagedSourceTrust::UserProvided);
    assert_ne!(github.descriptor().trust, ManagedSourceTrust::Official);
    // Not a GitHub host: refused rather than relabelled.
    assert!(CustomDatSource::github("https://example.test/a.dat", "x").is_err());
    // After activation the stored snapshot still says user-provided.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = CustomDatLifecycle::new(&github, dir.path()).unwrap();
    // The fixture bypasses DNS only through the injected resolver.
    let resolver = StaticResolver::new().with(
        "raw.githubusercontent.com",
        &["8.8.4.4".parse::<IpAddr>().unwrap()],
    );
    let candidate = lifecycle
        .download_with(&resolver, &Fixture::ok(dat("1"), "e1"))
        .unwrap();
    let active = lifecycle.activate(&candidate).unwrap().active;
    assert_eq!(active.trust, ManagedSourceTrust::UserProvided);
}

// --- lifecycle -----------------------------------------------------------

#[test]
fn download_validates_and_stages_but_only_explicit_activation_publishes() {
    let dir = tempfile::tempdir().unwrap();
    let source = source();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let transport = Fixture::ok(dat("1"), "e1");
    let candidate = lifecycle.download_with(&public(), &transport).unwrap();
    assert!(
        lifecycle.active().unwrap().is_none(),
        "download must not activate"
    );
    assert_eq!(candidate.validation.entry_count, 1);
    assert_eq!(candidate.validation.revision.as_deref(), Some("1"));
    assert!(!candidate.validation.extracted_from_archive);
    let preview = lifecycle.preview(&candidate).unwrap();
    assert!(preview.changed && preview.old.is_none());
    let result = lifecycle.activate(&candidate).unwrap();
    assert_eq!(
        lifecycle.active().unwrap().unwrap().sha256,
        candidate.candidate.snapshot.sha256
    );
    assert_eq!(result.active.trust, ManagedSourceTrust::UserProvided);
    assert_eq!(
        lifecycle.store().active_snapshot_bytes().unwrap().unwrap(),
        dat("1")
    );
}

#[test]
fn zip_release_assets_are_extracted_and_the_dat_itself_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let source = source();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let archive = zip_of(&[("README.txt", b"hi"), ("pack/catalogue.dat", &dat("9"))]);
    let candidate = lifecycle
        .download_with(&public(), &Fixture::ok(archive, "e9"))
        .unwrap();
    assert!(candidate.validation.extracted_from_archive);
    lifecycle.activate(&candidate).unwrap();
    assert_eq!(
        lifecycle.store().active_snapshot_bytes().unwrap().unwrap(),
        dat("9"),
        "the snapshot is the DAT, not the wrapper archive"
    );
}

#[test]
fn failed_validation_leaves_the_active_snapshot_and_staging_clean() {
    let dir = tempfile::tempdir().unwrap();
    let source = source();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let good = lifecycle
        .download_with(&public(), &Fixture::ok(dat("1"), "e1"))
        .unwrap();
    lifecycle.activate(&good).unwrap();
    let before = lifecycle.active().unwrap().unwrap().sha256;
    for body in [
        b"this is not a DAT at all".to_vec(),
        br#"<?xml version="1.0"?><datafile><header><name>Empty</name></header></datafile>"#
            .to_vec(),
    ] {
        assert!(
            lifecycle
                .download_with(&public(), &Fixture::ok(body, "bad"))
                .is_err()
        );
    }
    assert_eq!(lifecycle.active().unwrap().unwrap().sha256, before);
    assert!(
        staging_is_empty(dir.path()),
        "failed downloads must not leave staged files"
    );
}

#[test]
fn update_check_reports_available_unchanged_and_offline_without_downloading() {
    let dir = tempfile::tempdir().unwrap();
    let source = source();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let first = Fixture::ok(dat("1"), "e1");
    let candidate = lifecycle.download_with(&public(), &first).unwrap();
    lifecycle.activate(&candidate).unwrap();
    // Same validator: unchanged. New validator: available.
    let unchanged = lifecycle
        .check_update_with(false, &public(), &Fixture::ok(dat("1"), "e1"))
        .unwrap();
    assert!(matches!(unchanged, UpdateCheck::Unchanged { .. }));
    let newer = Fixture::ok(dat("2"), "e2");
    let available = lifecycle
        .check_update_with(false, &public(), &newer)
        .unwrap();
    assert!(matches!(available, UpdateCheck::Available { .. }));
    assert_eq!(*newer.fetches.borrow(), 0, "a check never downloads");
    // Offline never touches the transport or the resolver.
    let offline = lifecycle
        .check_update_with(true, &StaticResolver::new(), &Fixture::default())
        .unwrap();
    assert!(matches!(offline, UpdateCheck::Offline { active: Some(_) }));
    // Updating is explicit: download then activate; the old one goes to history.
    let update = lifecycle.download_with(&public(), &newer).unwrap();
    assert_eq!(
        lifecycle.active().unwrap().unwrap().sha256,
        candidate.candidate.snapshot.sha256,
        "an update never replaces the active DAT by itself"
    );
    lifecycle.activate(&update).unwrap();
    assert_eq!(lifecycle.history().unwrap().len(), 1);
}

#[test]
fn history_and_rollback_use_the_managed_store() {
    let dir = tempfile::tempdir().unwrap();
    let source = source();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let one = lifecycle
        .download_with(&public(), &Fixture::ok(dat("1"), "e1"))
        .unwrap();
    lifecycle.activate(&one).unwrap();
    let two = lifecycle
        .download_with(&public(), &Fixture::ok(dat("2"), "e2"))
        .unwrap();
    lifecycle.activate(&two).unwrap();
    let history = lifecycle.history().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].sha256, one.candidate.snapshot.sha256);
    let rolled = lifecycle.rollback(&history[0].sha256).unwrap();
    assert_eq!(rolled.active.sha256, one.candidate.snapshot.sha256);
    assert_eq!(
        lifecycle.store().active_snapshot_bytes().unwrap().unwrap(),
        dat("1")
    );
    assert!(lifecycle.rollback(&"0".repeat(64)).is_err());
}

#[test]
fn a_stale_candidate_cannot_replace_newer_state() {
    let dir = tempfile::tempdir().unwrap();
    let source = source();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let stale = lifecycle
        .download_with(&public(), &Fixture::ok(dat("1"), "e1"))
        .unwrap();
    let newer = lifecycle
        .download_with(&public(), &Fixture::ok(dat("2"), "e2"))
        .unwrap();
    lifecycle.activate(&newer).unwrap();
    let error = lifecycle.activate(&stale).unwrap_err().to_string();
    assert!(error.contains("stale"), "{error}");
    assert_eq!(
        lifecycle.active().unwrap().unwrap().sha256,
        newer.candidate.snapshot.sha256
    );
    // A candidate from another source is never accepted.
    let other = CustomDatSource::https("https://example.test/other.dat", "Other").unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let other_lifecycle = CustomDatLifecycle::new(&other, other_dir.path()).unwrap();
    assert!(other_lifecycle.activate(&newer).is_err());
}

#[test]
fn local_sources_import_offline_without_modifying_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("mine.dat");
    fs::write(&file, dat("3")).unwrap();
    let source = CustomDatSource::local(&file, "Mine").unwrap();
    let lifecycle = CustomDatLifecycle::new(&source, &dir.path().join("managed")).unwrap();
    let candidate = lifecycle.download().unwrap();
    assert_eq!(candidate.trust, DatAcquisitionTrust::UserProvided);
    lifecycle.activate(&candidate).unwrap();
    assert_eq!(fs::read(&file).unwrap(), dat("3"));
    assert!(
        lifecycle.check_update(false).is_err(),
        "a local file has no remote update"
    );
}

// --- bounded extraction --------------------------------------------------

#[test]
fn extraction_is_bounded_and_refuses_unsafe_archives() {
    let one = dat("1");
    let limits = ExtractLimits {
        max_members: 2,
        max_member_bytes: 1024 * 1024,
    };
    // Too many members.
    let many = zip_of(&[("a.txt", b"1"), ("b.txt", b"2"), ("c.dat", &one)]);
    assert!(extract_dat_payload(&many, &limits).is_err());
    // Traversal and absolute names.
    for name in [
        "../evil.dat",
        "a/../../evil.dat",
        "/abs.dat",
        "dir\\evil.dat",
    ] {
        let bad = zip_of(&[(name, &one)]);
        let error = extract_dat_payload(&bad, &ExtractLimits::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsafe"), "{name}: {error}");
    }
    // Oversized member.
    let tiny = ExtractLimits {
        max_members: 8,
        max_member_bytes: 16,
    };
    assert!(extract_dat_payload(&zip_of(&[("big.dat", &one)]), &tiny).is_err());
    // Ambiguous and empty archives.
    let two = zip_of(&[("a.dat", &one), ("b.dat", &one)]);
    assert!(extract_dat_payload(&two, &ExtractLimits::default()).is_err());
    assert!(
        extract_dat_payload(&zip_of(&[("notes.txt", b"x")]), &ExtractLimits::default()).is_err()
    );
    assert!(extract_dat_payload(b"PK\x03\x04garbage", &ExtractLimits::default()).is_err());
    // A well-formed single-DAT archive works and is returned byte for byte.
    assert_eq!(
        extract_dat_payload(&zip_of(&[("ok/a.dat", &one)]), &ExtractLimits::default()).unwrap(),
        one
    );
}

// --- redaction -----------------------------------------------------------

#[test]
fn secrets_are_redacted_everywhere_a_caller_can_see_them() {
    let url = format!("https://example.test/a.dat?token={TOKEN}");
    let source = CustomDatSource::https(&url, "Tokenised").unwrap();
    assert!(!source.display_address().contains(TOKEN));
    assert!(!format!("{source:?}").contains(TOKEN));
    assert!(!redact_url(&format!("https://user:{TOKEN}@example.test/a?x={TOKEN}")).contains(TOKEN));
    // A transport error that carries the request URL is scrubbed.
    let dir = tempfile::tempdir().unwrap();
    let lifecycle = CustomDatLifecycle::new(&source, dir.path()).unwrap();
    let failing = Fixture {
        fail_with: Some(format!("connect error for {url} refused")),
        ..Fixture::default()
    };
    let error = lifecycle
        .download_with(&public(), &failing)
        .unwrap_err()
        .to_string();
    assert!(!error.contains(TOKEN), "{error}");
    let error = lifecycle
        .check_update_with(false, &public(), &failing)
        .unwrap_err()
        .to_string();
    assert!(!error.contains(TOKEN), "{error}");
}

// --- registry ------------------------------------------------------------

#[test]
fn registry_persists_rejects_duplicates_and_never_touches_local_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sources.json");
    let local_file = dir.path().join("mine.dat");
    fs::write(&local_file, dat("1")).unwrap();
    let mut registry = CustomDatSourceRegistry::default();
    registry.add(source()).unwrap();
    registry
        .add(CustomDatSource::local(&local_file, "Mine").unwrap())
        .unwrap();
    assert!(registry.add(source()).is_err(), "duplicate source");
    registry.save(&path).unwrap();
    let loaded = CustomDatSourceRegistry::load(&path).unwrap();
    assert_eq!(loaded, registry);
    let id = loaded.sources()[0].id.clone();
    let mut loaded = loaded;
    assert!(loaded.remove(&id));
    assert!(!loaded.remove(&id));
    assert!(
        local_file.exists(),
        "removing a registration keeps the user's file"
    );
    assert!(
        CustomDatSourceRegistry::load(&dir.path().join("absent.json"))
            .unwrap()
            .sources()
            .is_empty()
    );
}

#[test]
fn registry_is_bounded_and_tampered_files_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = CustomDatSourceRegistry::default();
    for index in 0..CUSTOM_DAT_MAX_SOURCES {
        registry
            .add(CustomDatSource::https(&format!("https://example.test/{index}.dat"), "s").unwrap())
            .unwrap();
    }
    assert!(
        registry
            .add(CustomDatSource::https("https://example.test/overflow.dat", "s").unwrap())
            .is_err()
    );
    // Tampering: an official trust claim, a mismatched ID, an http URL.
    let path = dir.path().join("sources.json");
    let mut one = CustomDatSourceRegistry::default();
    one.add(source()).unwrap();
    one.save(&path).unwrap();
    let body = fs::read_to_string(&path).unwrap();
    for tampered in [
        body.replace("user_provided", "official"),
        body.replace(
            "https://example.test/catalogue.dat",
            "https://example.test/other.dat",
        ),
        body.replace("https://example.test", "http://example.test"),
        "{\"schema\":1,\"sources\":".to_string(),
    ] {
        fs::write(&path, tampered).unwrap();
        assert!(CustomDatSourceRegistry::load(&path).is_err());
    }
}
