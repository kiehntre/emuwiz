use super::*;
use crate::diagnostics::profiles::LinuxEmulatorInstallationEvidence;

fn install(name: &str) -> LinuxEmulatorInstallationEvidence {
    LinuxEmulatorInstallationEvidence {
        emulator: name.to_string(),
        installation_form: "Native/PATH".to_string(),
        executable: None,
        profile: None,
        detail: "found".to_string(),
    }
}

fn mame_dat(version: Option<&str>) -> ArcadeDatCatalogueVersion {
    ArcadeDatCatalogueVersion {
        ecosystem: DatEcosystem::MAMEArcade,
        version_header: version.map(str::to_string),
    }
}

fn fbneo_dat(version: Option<&str>) -> ArcadeDatCatalogueVersion {
    ArcadeDatCatalogueVersion {
        ecosystem: DatEcosystem::FBNeo,
        version_header: version.map(str::to_string),
    }
}

// --- MAME version parsing --------------------------------------------------

#[test]
fn parses_representative_mame_versions() {
    for (input, expected) in [
        ("0.270", "0.270"),
        ("MAME v0.270 (mame0270)", "0.270"),
        ("0.216 (mame0216-154-gabddfb0404c-dirty)", "0.216"),
        ("MAME 0.106u2", "0.106u2"),
        ("0.37b16", "0.37b16"),
        ("mame0270\n0.270 (mame0270)\nCopyright...", "0.270"),
    ] {
        let parsed = MameVersion::parse(input).unwrap_or_else(|| panic!("{input:?} should parse"));
        assert_eq!(parsed.display(), expected, "for {input:?}");
    }
}

#[test]
fn malformed_mame_output_does_not_parse() {
    for input in [
        "",
        "unrelated text",
        "1.270",
        "0.",
        "0.abc",
        "0.27000",   // 5 digits
        "0.106u",    // no update number
        "0.106u123", // 3-digit update
        "version 270",
    ] {
        assert!(
            MameVersion::parse(input).is_none(),
            "{input:?} must not parse"
        );
    }
}

#[test]
fn mame_versions_order_across_beta_release_and_update_phases() {
    let versions = [
        MameVersion::parse("0.37b16").unwrap(),
        MameVersion::parse("0.37").unwrap(),
        MameVersion::parse("0.106u2").unwrap(),
        MameVersion::parse("0.107").unwrap(),
        MameVersion::parse("0.270").unwrap(),
    ];
    for pair in versions.windows(2) {
        assert!(pair[0] < pair[1], "{:?} should be < {:?}", pair[0], pair[1]);
    }
    assert_eq!(
        MameVersion::parse("0.270").unwrap(),
        MameVersion::parse("MAME v0.270 (mame0270)").unwrap()
    );
}

// --- FBNeo version parsing ---------------------------------------------

#[test]
fn parses_representative_fbneo_versions() {
    for (input, expected) in [
        ("v1.0.0.02", "1.0.0.2"),
        ("1.0.0.3", "1.0.0.3"),
        ("FBNeo 1.0.0.3 260723 GIT7a28a7d", "1.0.0.3"),
        ("v1.0.0", "1.0.0"),
    ] {
        let parsed = FbneoVersion::parse(input).unwrap_or_else(|| panic!("{input:?} should parse"));
        assert_eq!(parsed.display(), expected, "for {input:?}");
    }
}

#[test]
fn fbneo_unsafe_formats_do_not_parse() {
    for input in [
        "",
        "FBNeo",
        "260723", // bare date, single token, no dots
        "GIT7a28a7d",
        "1",         // single component
        "1.0.0.0.0", // 5 components
        "1.0.x",     // non-numeric
        "20230727",  // date, no dots
    ] {
        assert!(
            FbneoVersion::parse(input).is_none(),
            "{input:?} must not parse"
        );
    }
}

#[test]
fn fbneo_versions_order_numerically_not_lexically() {
    assert!(FbneoVersion::parse("v1.0.0.02").unwrap() < FbneoVersion::parse("1.0.0.3").unwrap());
    assert!(FbneoVersion::parse("1.0.0").unwrap() < FbneoVersion::parse("1.0.0.1").unwrap());
    assert!(FbneoVersion::parse("1.10.0").unwrap() > FbneoVersion::parse("1.9.0").unwrap());
}

// --- compatibility ---------------------------------------------------

#[test]
fn exact_version_match_is_current() {
    let readiness = arcade_emulator_dat_readiness(
        ArcadeEmulator::Mame,
        true,
        Some("0.270 (mame0270)"),
        &mame_dat(Some("0.270")),
    );
    assert_eq!(
        readiness.compatibility,
        ArcadeDatVersionCompatibility::Matching
    );
    assert_eq!(readiness.emulator_version.as_deref(), Some("0.270"));
    assert_eq!(readiness.dat_revision.as_deref(), Some("0.270"));
    assert_eq!(readiness.compatibility.label(), "Current");
}

#[test]
fn a_dat_from_an_earlier_mame_is_older_dat() {
    let readiness = arcade_emulator_dat_readiness(
        ArcadeEmulator::Mame,
        true,
        Some("0.270"),
        &mame_dat(Some("0.265")),
    );
    assert_eq!(
        readiness.compatibility,
        ArcadeDatVersionCompatibility::DatOlderThanEmulator
    );
    assert_eq!(readiness.compatibility.label(), "Older DAT");
}

#[test]
fn a_dat_from_a_later_mame_is_newer_dat() {
    let readiness = arcade_emulator_dat_readiness(
        ArcadeEmulator::Mame,
        true,
        Some("0.265"),
        &mame_dat(Some("0.270")),
    );
    assert_eq!(
        readiness.compatibility,
        ArcadeDatVersionCompatibility::DatNewerThanEmulator
    );
    assert_eq!(readiness.compatibility.label(), "Newer DAT");
}

#[test]
fn a_detected_emulator_with_no_parseable_version_is_unknown() {
    let readiness = arcade_emulator_dat_readiness(
        ArcadeEmulator::Mame,
        true,
        Some("MAME build: some unexpected new shape"),
        &mame_dat(Some("0.270")),
    );
    assert_eq!(readiness.emulator_detected, true);
    assert_eq!(readiness.emulator_version, None);
    assert_eq!(
        readiness.compatibility,
        ArcadeDatVersionCompatibility::Unknown
    );
}

#[test]
fn a_missing_dat_revision_is_unknown_not_a_mismatch() {
    let readiness =
        arcade_emulator_dat_readiness(ArcadeEmulator::Mame, true, Some("0.270"), &mame_dat(None));
    assert_eq!(readiness.dat_revision, None);
    assert_eq!(
        readiness.compatibility,
        ArcadeDatVersionCompatibility::Unknown
    );
}

#[test]
fn fbneo_falls_to_unknown_when_the_emulator_version_is_not_a_clean_dotted_number() {
    let readiness = arcade_emulator_dat_readiness(
        ArcadeEmulator::Fbneo,
        true,
        Some("FinalBurn Neo (SDL) - dev build"),
        &fbneo_dat(Some("v1.0.0.02")),
    );
    assert_eq!(readiness.emulator_version, None);
    assert_eq!(
        readiness.compatibility,
        ArcadeDatVersionCompatibility::Unknown
    );
}

#[test]
fn fbneo_compares_clean_dotted_versions_when_both_sides_parse() {
    let older = arcade_emulator_dat_readiness(
        ArcadeEmulator::Fbneo,
        true,
        Some("FBNeo 1.0.0.3 260723 GIT7a28a7d"),
        &fbneo_dat(Some("v1.0.0.02")),
    );
    assert_eq!(
        older.compatibility,
        ArcadeDatVersionCompatibility::DatOlderThanEmulator
    );
    assert_eq!(older.emulator_version.as_deref(), Some("1.0.0.3"));
}

#[test]
fn a_mame_dat_never_authorises_an_fbneo_comparison_and_vice_versa() {
    let cross = arcade_emulator_dat_readiness(
        ArcadeEmulator::Fbneo,
        true,
        Some("1.0.0.3"),
        &mame_dat(Some("0.270")),
    );
    assert_eq!(
        cross.compatibility,
        ArcadeDatVersionCompatibility::NotApplicable
    );
}

// --- assembly / source scoping -------------------------------------

#[test]
fn mame_and_fbneo_readiness_stay_source_separated() {
    let installs = [install("MAME"), install("FinalBurn Neo")];
    let catalogues = [mame_dat(Some("0.270")), fbneo_dat(Some("v1.0.0.02"))];
    let readiness = arcade_dat_version_readiness(&installs, &catalogues, &[]);
    assert_eq!(readiness.len(), 2);

    let mame = readiness
        .iter()
        .find(|item| item.emulator == ArcadeEmulator::Mame)
        .unwrap();
    assert_eq!(mame.dat_ecosystem, DatEcosystem::MAMEArcade);
    assert_eq!(mame.dat_revision.as_deref(), Some("0.270"));

    let fbneo = readiness
        .iter()
        .find(|item| item.emulator == ArcadeEmulator::Fbneo)
        .unwrap();
    assert_eq!(fbneo.dat_ecosystem, DatEcosystem::FBNeo);
    assert_eq!(fbneo.dat_revision.as_deref(), Some("v1.0.0.02"));
}

#[test]
fn a_detected_emulator_with_no_configured_catalogue_still_gets_an_honest_line() {
    let installs = [install("MAME")];
    let readiness = arcade_dat_version_readiness(&installs, &[], &[]);
    assert_eq!(readiness.len(), 1);
    assert_eq!(readiness[0].emulator, ArcadeEmulator::Mame);
    assert!(readiness[0].emulator_detected);
    assert_eq!(readiness[0].emulator_version, None);
    assert_eq!(readiness[0].dat_revision, None);
    assert_eq!(
        readiness[0].compatibility,
        ArcadeDatVersionCompatibility::Unknown
    );
}

#[test]
fn a_configured_catalogue_with_no_detected_emulator_reports_not_detected() {
    let readiness = arcade_dat_version_readiness(&[], &[mame_dat(Some("0.270"))], &[]);
    assert_eq!(readiness.len(), 1);
    assert!(!readiness[0].emulator_detected);
    assert_eq!(
        readiness[0].compatibility,
        ArcadeDatVersionCompatibility::Unknown
    );
}

#[test]
fn a_supplied_version_output_is_used_for_the_matching_emulator_only() {
    let installs = [install("MAME"), install("FinalBurn Neo")];
    let catalogues = [mame_dat(Some("0.270")), fbneo_dat(Some("1.0.0.3"))];
    let readiness = arcade_dat_version_readiness(
        &installs,
        &catalogues,
        &[(ArcadeEmulator::Mame, "0.270 (mame0270)".to_string())],
    );
    let mame = readiness
        .iter()
        .find(|item| item.emulator == ArcadeEmulator::Mame)
        .unwrap();
    assert_eq!(mame.emulator_version.as_deref(), Some("0.270"));
    assert_eq!(mame.compatibility, ArcadeDatVersionCompatibility::Matching);

    let fbneo = readiness
        .iter()
        .find(|item| item.emulator == ArcadeEmulator::Fbneo)
        .unwrap();
    assert_eq!(fbneo.emulator_version, None); // no output supplied for FBNeo
    assert_eq!(fbneo.compatibility, ArcadeDatVersionCompatibility::Unknown);
}

// --- Doctor findings -------------------------------------------------

#[test]
fn doctor_findings_are_advisory_info_and_never_call_the_rom_set_broken() {
    let readiness = [
        arcade_emulator_dat_readiness(
            ArcadeEmulator::Mame,
            true,
            Some("0.270"),
            &mame_dat(Some("0.265")),
        ),
        arcade_emulator_dat_readiness(
            ArcadeEmulator::Fbneo,
            true,
            None,
            &fbneo_dat(Some("v1.0.0.02")),
        ),
    ];
    let findings = findings_from_arcade_dat_version(&readiness);
    assert_eq!(findings.len(), 2);
    for finding in &findings {
        assert_eq!(finding.severity, DoctorSeverity::Info);
        assert_eq!(finding.category, DoctorCategory::Emulators);
        let text = format!("{} {}", finding.title, finding.explanation).to_lowercase();
        assert!(!text.contains("broken"));
        assert!(!text.contains("incomplete"));
        assert!(!text.contains("missing rom"));
        assert!(!text.contains("corrupt"));
    }
    let older = findings
        .iter()
        .find(|f| f.id == "arcade_dat_version.mame")
        .unwrap();
    assert_eq!(older.title, "MAME listxml DAT is older than installed MAME");
    assert!(
        older
            .explanation
            .contains("does not by itself change ROM-set completeness")
    );
    assert!(
        older
            .evidence
            .iter()
            .any(|e| e == "Compatibility: Older DAT")
    );

    let fbneo = findings
        .iter()
        .find(|f| f.id == "arcade_dat_version.fbneo")
        .unwrap();
    assert_eq!(
        fbneo.title,
        "FinalBurn Neo DAT / FinalBurn Neo version compatibility is unknown"
    );
    assert!(
        fbneo
            .explanation
            .contains("the installed FinalBurn Neo version could not be determined")
    );
}

#[test]
fn doctor_finding_for_a_matching_pair_reads_as_current() {
    let readiness = [arcade_emulator_dat_readiness(
        ArcadeEmulator::Mame,
        true,
        Some("0.270"),
        &mame_dat(Some("0.270")),
    )];
    let findings = findings_from_arcade_dat_version(&readiness);
    assert_eq!(findings[0].title, "MAME listxml DAT matches installed MAME");
    assert!(
        findings[0]
            .evidence
            .iter()
            .any(|e| e == "Installed MAME: 0.270")
    );
}

#[test]
fn doctor_finding_says_version_unknown_when_no_output_was_captured() {
    let readiness = arcade_dat_version_readiness(&[install("MAME")], &[], &[]);
    let findings = findings_from_arcade_dat_version(&readiness);
    assert_eq!(findings.len(), 1);
    assert!(
        findings[0]
            .evidence
            .iter()
            .any(|e| e == "Installed MAME: detected, version unknown")
    );
    assert!(
        findings[0]
            .evidence
            .iter()
            .any(|e| e == "MAME listxml DAT revision: unknown")
    );
    // Both the version and the DAT revision are unknown; the finding names
    // the emulator-version reason first (a captured `mame -version` is what
    // would resolve it).
    assert!(
        findings[0]
            .explanation
            .to_lowercase()
            .contains("installed mame version could not be determined")
    );
}
