//! Tests for the read-only Doctor findings over the verified-identity fact
//! cache. Every fixture builds `ArchiveIdentityFactStatus` values by hand;
//! the mapping rules are what is under test.

use super::*;
use crate::game_identity::IdentityConfidence;
use crate::verified_identity_cache::PersistedIdentityFact;

fn fact(kind: IdentityKind, value: &str) -> PersistedIdentityFact {
    PersistedIdentityFact {
        archive_id: 1,
        kind,
        value: value.to_string(),
        confidence: IdentityConfidence::ExactBytes,
        method: None,
        member_path: None,
        observed_at: "2026-01-01T00:00:00Z".to_string(),
        file_device: 66,
        file_inode: 100,
        file_size_bytes: 5_000,
        file_modified_unix_seconds: Some(1_700_000_000),
    }
}

fn status(
    platform_id: Option<&str>,
    facts: Vec<(PersistedIdentityFact, IdentityFactFreshness)>,
) -> ArchiveIdentityFactStatus {
    ArchiveIdentityFactStatus {
        archive_id: 1,
        display_name: "Some Game".to_string(),
        platform_id: platform_id.map(str::to_string),
        facts,
    }
}

fn ids(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|finding| finding.id.as_str()).collect()
}

#[test]
fn a_missing_ps1_serial_is_a_truthful_info_finding() {
    let findings = findings_from_verified_identity_facts(&[status(Some("PSX"), Vec::new())]);
    assert_eq!(ids(&findings), vec!["emulators.verified_identity_missing"]);
    let finding = &findings[0];
    assert_eq!(finding.severity, DoctorSeverity::Info);
    assert_eq!(finding.category, DoctorCategory::Emulators);
    assert!(finding.explanation.contains("PlayStation serial"));
    assert!(finding.explanation.contains("DuckStation"));
    assert!(finding.explanation.to_lowercase().contains("blocked"));
    assert!(
        !finding.explanation.to_lowercase().contains("corrupt"),
        "an absent launch ID is not corruption"
    );
    assert_eq!(
        finding.affected.as_ref().map(|p| p.display.as_str()),
        Some("Some Game")
    );
}

#[test]
fn a_current_fact_produces_no_finding() {
    let findings = findings_from_verified_identity_facts(&[status(
        Some("PSX"),
        vec![(
            fact(IdentityKind::Ps1Serial, "SCUS-94103"),
            IdentityFactFreshness::Current,
        )],
    )]);
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn a_stale_fact_is_reported_as_out_of_date_not_as_missing() {
    let findings = findings_from_verified_identity_facts(&[status(
        Some("PSX"),
        vec![(
            fact(IdentityKind::Ps1Serial, "SCUS-94103"),
            IdentityFactFreshness::Stale,
        )],
    )]);
    assert_eq!(ids(&findings), vec!["emulators.verified_identity_stale"]);
    assert!(findings[0].explanation.contains("changed since"));
    assert!(findings[0].explanation.contains("re-verify"));
}

#[test]
fn an_unknown_freshness_fact_is_reported_distinctly() {
    let findings = findings_from_verified_identity_facts(&[status(
        Some("PSX"),
        vec![(
            fact(IdentityKind::Ps1Serial, "SCUS-94103"),
            IdentityFactFreshness::Unknown,
        )],
    )]);
    assert_eq!(
        ids(&findings),
        vec!["emulators.verified_identity_unknown_freshness"]
    );
}

#[test]
fn ps2_distinguishes_the_launch_serial_from_the_patch_crc() {
    let findings = findings_from_verified_identity_facts(&[status(Some("PS2"), Vec::new())]);
    assert_eq!(findings.len(), 2, "{findings:?}");
    let serial = findings
        .iter()
        .find(|f| f.explanation.contains("PlayStation 2 serial"))
        .expect("a serial finding");
    let crc = findings
        .iter()
        .find(|f| f.explanation.contains("PCSX2 executable CRC"))
        .expect("a CRC finding");
    assert!(
        serial
            .explanation
            .to_lowercase()
            .contains("launch will remain blocked"),
        "the serial gates launch: {}",
        serial.explanation
    );
    assert!(
        crc.explanation.contains("patch and cheat compatibility")
            && crc
                .explanation
                .contains("launch itself does not depend on it"),
        "the CRC gates patch/cheat compatibility only: {}",
        crc.explanation
    );
}

#[test]
fn an_unresolved_platform_produces_no_finding() {
    for platform_id in [None, Some(""), Some("totally unknown thing")] {
        let findings = findings_from_verified_identity_facts(&[status(platform_id, Vec::new())]);
        assert!(findings.is_empty(), "{platform_id:?} -> {findings:?}");
    }
}

#[test]
fn a_platform_without_a_launch_identity_requirement_produces_no_finding() {
    for platform_id in ["SNES", "NES", "Genesis", "Nintendo 64", "Sega Saturn"] {
        let findings =
            findings_from_verified_identity_facts(&[status(Some(platform_id), Vec::new())]);
        assert!(findings.is_empty(), "{platform_id} -> {findings:?}");
    }
}

#[test]
fn xbox_360_warns_only_when_both_the_title_id_and_media_id_are_absent() {
    let neither = findings_from_verified_identity_facts(&[status(Some("Xbox 360"), Vec::new())]);
    assert_eq!(ids(&neither), vec!["emulators.verified_identity_missing"]);
    assert!(
        neither[0]
            .explanation
            .contains("Xbox 360 title ID or Xbox 360 media ID")
    );

    // Just the media ID, and current: the requirement is met.
    let media_only = findings_from_verified_identity_facts(&[status(
        Some("Xbox 360"),
        vec![(
            fact(IdentityKind::XexMediaId, "1E1F2A3B"),
            IdentityFactFreshness::Current,
        )],
    )]);
    assert!(media_only.is_empty(), "{media_only:?}");

    // One present but stale: no missing finding, but the staleness is noted.
    let stale_media = findings_from_verified_identity_facts(&[status(
        Some("Xbox 360"),
        vec![(
            fact(IdentityKind::XexMediaId, "1E1F2A3B"),
            IdentityFactFreshness::Stale,
        )],
    )]);
    assert_eq!(ids(&stale_media), vec!["emulators.verified_identity_stale"]);
}

#[test]
fn gamecube_and_wii_require_a_dolphin_game_id() {
    for platform_id in ["GameCube", "Wii"] {
        let findings =
            findings_from_verified_identity_facts(&[status(Some(platform_id), Vec::new())]);
        assert_eq!(ids(&findings), vec!["emulators.verified_identity_missing"]);
        assert!(findings[0].explanation.contains("Dolphin Game ID"));
        assert!(findings[0].explanation.contains("Dolphin"));
    }
}

#[test]
fn every_finding_is_info_and_never_calls_a_game_corrupt() {
    let statuses = vec![
        status(Some("PSX"), Vec::new()),
        status(Some("PS2"), Vec::new()),
        status(Some("PSP"), Vec::new()),
        status(Some("PS3"), Vec::new()),
        status(Some("Xbox"), Vec::new()),
        status(Some("Xbox 360"), Vec::new()),
        status(Some("Dreamcast"), Vec::new()),
        status(
            Some("GameCube"),
            vec![(
                fact(IdentityKind::DolphinGameId, "GALE01"),
                IdentityFactFreshness::Stale,
            )],
        ),
    ];
    let findings = findings_from_verified_identity_facts(&statuses);
    assert!(!findings.is_empty());
    for finding in &findings {
        assert_eq!(finding.severity, DoctorSeverity::Info);
        let haystack = format!(
            "{} {} {}",
            finding.title,
            finding.explanation,
            finding.why_it_matters.clone().unwrap_or_default()
        )
        .to_lowercase();
        assert!(!haystack.contains("corrupt"), "{}", finding.explanation);
    }
}

#[test]
fn findings_carry_the_archive_as_their_affected_resource() {
    let mut a = status(Some("PSX"), Vec::new());
    a.display_name = "Game A".to_string();
    let mut b = status(Some("PS3"), Vec::new());
    b.display_name = "Game B".to_string();
    b.archive_id = 2;
    let findings = findings_from_verified_identity_facts(&[a, b]);
    let mut affected: Vec<&str> = findings
        .iter()
        .filter_map(|f| f.affected.as_ref().map(|p| p.display.as_str()))
        .collect();
    affected.sort();
    assert_eq!(affected, vec!["Game A", "Game B"]);
}
