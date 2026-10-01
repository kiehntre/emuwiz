use super::*;

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn no_view_flag_selects_every_view_and_flags_select_only_theirs() {
    assert!(parse(args(&["a.nes"])).unwrap().views.provenance);
    let narrow = parse(args(&["a.nes", "--readiness", "--json"])).unwrap();
    assert!(narrow.json && narrow.views.readiness);
    assert!(!narrow.views.identity && !narrow.views.media && !narrow.views.evidence);
}

#[test]
fn bad_arguments_are_refused_with_a_reason() {
    for bad in [
        &[][..],
        &["--bogus", "a"],
        &["a", "b"],
        &["--platform"],
        &["--root"],
        &["--", "a", "b"],
    ] {
        assert!(parse(args(bad)).is_err(), "{bad:?}");
    }
    // `--` allows a path that starts with a dash.
    assert_eq!(
        parse(args(&["--", "-odd.nes"])).unwrap().path,
        PathBuf::from("-odd.nes")
    );
}

#[test]
fn only_bytes_derived_facts_are_verified() {
    use IdentityConfidence::*;
    use IdentityStatus::*;
    assert_eq!(class(Verified, ExactBytes), "verified_fact");
    assert_eq!(class(Verified, StructuredMetadata), "verified_fact");
    // A hint echoed back as "Verified" and a filename are never facts.
    assert_eq!(class(Verified, CatalogueContext), "catalogue_context");
    assert_eq!(class(Verified, FilenameOnly), "filename_inference");
    assert_eq!(class(Candidate, FilenameOnly), "filename_inference");
    assert_eq!(class(Candidate, ExactBytes), "candidate");
    assert_eq!(class(Ambiguous, ExactBytes), "ambiguous");
    assert_eq!(class(Invalid, ExactBytes), "invalid");
    assert_eq!(class(ResourceLimitReached, Unavailable), "incomplete");
    assert_eq!(class(Missing, Unavailable), "unsupported_or_unknown");
    assert_eq!(class(Unsupported, Unavailable), "unsupported_or_unknown");
}

#[test]
fn a_checksum_or_platform_echo_is_not_an_identity() {
    assert_eq!(category(IdentityKind::LooseRomSha256), "checksum");
    assert_eq!(category(IdentityKind::Platform), "platform_context");
    assert_eq!(category(IdentityKind::NesHeader), "attribute");
    assert_eq!(category(IdentityKind::Ps2Serial), "identity");
    assert_eq!(category(IdentityKind::DolphinGameId), "identity");
}

#[test]
fn scummvm_directories_never_reach_the_external_detector() {
    let dir = Target {
        path: "/g".into(),
        path_is_utf8: true,
        kind: "directory",
        size_bytes: None,
    };
    let file = Target {
        path: "/g.zip".into(),
        path_is_utf8: true,
        kind: "file",
        size_bytes: Some(1),
    };
    for hint in ["scummvm", "ScummVM", " SCUMM VM "] {
        assert!(needs_external_detector(Some(hint), &dir), "{hint}");
        // Only a directory triggers the detector subprocess in the core.
        assert!(!needs_external_detector(Some(hint), &file), "{hint}");
    }
    for hint in [None, Some("ps2"), Some("gamecube"), Some("NES")] {
        assert!(!needs_external_detector(hint, &dir), "{hint:?}");
    }
}
