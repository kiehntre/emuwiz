//! Synthetic catalogues and ZIPs in a private temp directory only.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use archivefs_core::arcade_compat_check::*;
use archivefs_core::arcade_compatibility::ArcadeEmulatorCompatibility;
use archivefs_core::arcade_mame_compatibility::MameSetCompatibilityState;
use archivefs_core::arcade_recommendation::ArcadeRecommendationChoice;

const VERSION: &str = "MAME 0.280 (mame0280)";

fn crc(bytes: &[u8]) -> String {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    format!("{:08x}", !crc)
}

fn zip(path: &Path, members: &[(&str, &[u8])]) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for (name, bytes) in members {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn catalogue(dir: &Path, machines: &str) -> PathBuf {
    let path = dir.join("listxml.xml");
    std::fs::write(&path, format!("<mame build=\"0.280\">{machines}</mame>")).unwrap();
    path
}

fn family() -> String {
    format!(
        r#"<machine name="child" cloneof="parent" romof="parent">
             <rom name="c.bin" size="4" crc="{}"/></machine>
           <machine name="parent"><rom name="p.bin" size="4" crc="{}"/></machine>"#,
        crc(b"abcd"),
        crc(b"efgh")
    )
}

fn request(dir: &Path, version: &str) -> ArcadeCheckRequest {
    let exe = dir.join("mame");
    if !exe.exists() {
        std::fs::write(&exe, b"never executed").unwrap();
    }
    ArcadeCheckRequest {
        set_name: "child".into(),
        archive_path: dir.join("child.zip"),
        mame: Some(MameCheckInput {
            catalogue_path: dir.join("listxml.xml"),
            version_output: version.into(),
            executable_path: Some(exe),
        }),
        fbneo: None,
        user_preference: None,
    }
}

fn setup(machines: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    catalogue(dir.path(), machines);
    zip(&dir.path().join("child.zip"), &[("c.bin", b"abcd")]);
    zip(&dir.path().join("parent.zip"), &[("p.bin", b"efgh")]);
    dir
}

fn run(req: &ArcadeCheckRequest) -> Result<ArcadeCheckReport, ArcadeCheckError> {
    run_arcade_compat_check(req, &AtomicBool::new(false))
}

fn mame_state(report: &ArcadeCheckReport) -> Option<MameSetCompatibilityState> {
    match &report.summary.mame {
        ArcadeEmulatorCompatibility::Evaluated(r) => Some(r.state),
        _ => None,
    }
}

#[test]
fn complete_verified_family_recommends_mame_and_stays_storage_scoped() {
    let dir = setup(&family());
    let report = run(&request(dir.path(), VERSION)).unwrap();
    assert!(matches!(
        mame_state(&report),
        Some(
            MameSetCompatibilityState::Compatible
                | MameSetCompatibilityState::CompatibleWithWarnings
        )
    ));
    assert_eq!(
        report.recommendation.preferred,
        ArcadeRecommendationChoice::Mame
    );
    assert!(
        report
            .limits
            .iter()
            .any(|l| l.contains("not proof the game runs"))
    );
    assert!(report.provenance.is_current());
}

#[test]
fn wrong_checksum_is_incompatible_not_recommended() {
    let dir = setup(&family());
    zip(&dir.path().join("child.zip"), &[("c.bin", b"zzzz")]);
    let report = run(&request(dir.path(), VERSION)).unwrap();
    assert_eq!(
        mame_state(&report),
        Some(MameSetCompatibilityState::Incompatible)
    );
    assert_ne!(
        report.recommendation.preferred,
        ArcadeRecommendationChoice::Mame
    );
}

#[test]
fn missing_parent_archive_is_unknown_not_a_failure() {
    let dir = setup(&family());
    std::fs::remove_file(dir.path().join("parent.zip")).unwrap();
    let report = run(&request(dir.path(), VERSION)).unwrap();
    assert_eq!(
        report.provenance.missing_archives,
        vec!["parent".to_string()]
    );
    assert!(!matches!(
        mame_state(&report),
        Some(
            MameSetCompatibilityState::Compatible
                | MameSetCompatibilityState::CompatibleWithWarnings
                | MameSetCompatibilityState::Incompatible
        )
    ));
}

#[test]
fn absent_or_mismatched_version_evidence_is_insufficient() {
    let dir = setup(&family());
    for version in ["", "MAME 0.281 (mame0281)", "not mame"] {
        let report = run(&request(dir.path(), version)).unwrap();
        assert_eq!(
            report.recommendation.preferred,
            ArcadeRecommendationChoice::Unknown,
            "version {version:?}"
        );
        assert!(report.recommendation.play_now.is_none());
    }
}

#[test]
fn unsafe_names_non_zip_and_empty_requests_are_refused() {
    let dir = setup(&family());
    for bad in ["../x", "a/b", ".hidden", ""] {
        let mut req = request(dir.path(), VERSION);
        req.set_name = bad.into();
        assert_eq!(run(&req), Err(ArcadeCheckError::InvalidSetName));
    }
    let mut req = request(dir.path(), VERSION);
    req.mame = None;
    assert_eq!(run(&req), Err(ArcadeCheckError::NothingToCheck));
    let mut req = request(dir.path(), VERSION);
    std::fs::rename(dir.path().join("child.zip"), dir.path().join("child.7z")).unwrap();
    req.archive_path = dir.path().join("child.7z");
    assert!(matches!(run(&req), Err(ArcadeCheckError::Archive(_))));
}

#[cfg(unix)]
#[test]
fn a_symlinked_game_is_not_followed() {
    let dir = setup(&family());
    std::fs::rename(dir.path().join("child.zip"), dir.path().join("real.zip")).unwrap();
    std::os::unix::fs::symlink(dir.path().join("real.zip"), dir.path().join("child.zip")).unwrap();
    assert!(matches!(
        run(&request(dir.path(), VERSION)),
        Err(ArcadeCheckError::Archive(_))
    ));
}

#[test]
fn changing_an_input_invalidates_the_report() {
    let dir = setup(&family());
    let report = run(&request(dir.path(), VERSION)).unwrap();
    assert!(report.provenance.is_current());
    zip(
        &dir.path().join("parent.zip"),
        &[("p.bin", b"efgh"), ("extra", b"1")],
    );
    assert!(!report.provenance.is_current());
}

#[test]
fn a_catalogue_without_the_machine_gives_no_proof() {
    let dir = setup(r#"<machine name="other"><rom name="o" size="4" crc="00000000"/></machine>"#);
    let report = run(&request(dir.path(), VERSION)).unwrap();
    assert_ne!(
        report.recommendation.preferred,
        ArcadeRecommendationChoice::Mame
    );
}

#[test]
fn cancellation_stops_the_check() {
    let dir = setup(&family());
    let cancel = AtomicBool::new(true);
    assert_eq!(
        run_arcade_compat_check(&request(dir.path(), VERSION), &cancel),
        Err(ArcadeCheckError::Cancelled)
    );
}

#[test]
fn fbneo_with_a_named_core_and_dat_can_be_the_only_proven_emulator() {
    let dir = setup(&family());
    let core = dir.path().join("fbneo_libretro.so");
    std::fs::write(&core, b"not executed").unwrap();
    std::fs::write(
        dir.path().join("fbneo.dat"),
        format!(
            r#"<?xml version="1.0"?><datafile><header><name>FinalBurn Neo - Arcade Games</name><description>FinalBurn Neo</description><version>1.0.0</version></header>
<game name="child" cloneof="parent" romof="parent"><description>c</description><rom name="c.bin" size="4" crc="{}"/></game>
<game name="parent"><description>p</description><rom name="p.bin" size="4" crc="{}"/></game></datafile>"#,
            crc(b"abcd"),
            crc(b"efgh")
        ),
    )
    .unwrap();
    let mut req = request(dir.path(), "");
    req.fbneo = Some(FbNeoCheckInput {
        dat_path: dir.path().join("fbneo.dat"),
        core_path: Some(core),
        reported_version: None,
    });
    let report = run(&req).unwrap();
    // MAME has no captured version, so only FBNeo can be proven.
    assert_eq!(
        report.recommendation.preferred,
        ArcadeRecommendationChoice::FinalBurnNeo
    );
    // Without a named core FBNeo stays unknown.
    req.fbneo.as_mut().unwrap().core_path = None;
    assert_eq!(
        run(&req).unwrap().recommendation.preferred,
        ArcadeRecommendationChoice::Unknown
    );
}

#[test]
fn version_text_without_an_existing_executable_never_recommends_mame() {
    let dir = setup(&family());
    let mut req = request(dir.path(), VERSION);
    req.mame.as_mut().unwrap().executable_path = Some(dir.path().join("no-such-mame"));
    let report = run(&req).unwrap();
    assert_eq!(
        report.recommendation.preferred,
        ArcadeRecommendationChoice::Unknown
    );
    assert!(
        report
            .details
            .iter()
            .any(|d| d.contains("No existing MAME executable"))
    );
    req.mame.as_mut().unwrap().executable_path = None;
    assert_eq!(
        run(&req).unwrap().recommendation.preferred,
        ArcadeRecommendationChoice::Unknown
    );
}

#[test]
fn creating_a_named_but_absent_executable_invalidates_the_report() {
    let dir = setup(&family());
    let mut req = request(dir.path(), VERSION);
    let exe = dir.path().join("later-mame");
    req.mame.as_mut().unwrap().executable_path = Some(exe.clone());
    let report = run(&req).unwrap();
    assert!(report.provenance.is_current());
    std::fs::write(&exe, b"x").unwrap();
    assert!(!report.provenance.is_current());
}

#[test]
fn a_corrupt_support_archive_is_unobserved_not_a_failed_check() {
    let dir = setup(&family());
    std::fs::write(dir.path().join("parent.zip"), b"this is not a zip").unwrap();
    let report = run(&request(dir.path(), VERSION)).unwrap();
    assert_eq!(
        report.provenance.unreadable_archives,
        vec!["parent".to_string()]
    );
    assert!(!matches!(
        mame_state(&report),
        Some(
            MameSetCompatibilityState::Compatible
                | MameSetCompatibilityState::CompatibleWithWarnings
        )
    ));
    assert!(report.limits.iter().any(|l| l.contains("unobserved")));
    // The selected game's own corrupt ZIP is still a refusal, without paths.
    std::fs::write(dir.path().join("child.zip"), b"nope").unwrap();
    match run(&request(dir.path(), VERSION)) {
        Err(ArcadeCheckError::Archive(message)) => assert!(!message.contains('/')),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_machine_absent_from_the_catalogue_is_explained() {
    let dir = setup(r#"<machine name="other"><rom name="o" size="4" crc="00000000"/></machine>"#);
    let report = run(&request(dir.path(), VERSION)).unwrap();
    assert!(
        report
            .details
            .iter()
            .any(|d| d.contains("does not list a machine"))
    );
}
