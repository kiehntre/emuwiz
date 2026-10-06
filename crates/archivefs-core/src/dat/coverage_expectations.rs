//! Expected authoritative evidence by canonical platform.
//!
//! This is deliberately a mapping of *expected evidence*, not a registry of
//! installed sources and not a verification result.  A platform can therefore
//! be expected to have No-Intro coverage while no No-Intro source is active.
//! The later coverage projection can join this model to the existing source
//! and freshness state without changing authority rules.

use serde::{Deserialize, Serialize};

use super::model::DatEcosystem;

/// An authoritative ecosystem or official detector that can normally provide
/// identity evidence for a platform. Metadata providers and user DATs are
/// intentionally absent from this closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "ecosystem")]
pub enum ExpectedAuthoritativeSource {
    Dat(DatEcosystem),
    OfficialScummVmDetector,
}

impl ExpectedAuthoritativeSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dat(ecosystem) => ecosystem.label(),
            Self::OfficialScummVmDetector => "Official ScummVM detector",
        }
    }
}

/// The role of a source when more than one evidence ecosystem is relevant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageSourceRole {
    Primary,
    Secondary,
    Supplementary,
}

/// Typed rationale retained for later coverage explanations. Presentation
/// layers should translate these codes into user-facing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageRationale {
    SupportedCartridgeDat,
    SupportedOpticalDat,
    OfficialMameArcadeEvidence,
    OfficialScummVmDetectorEvidence,
    ExplicitTosecClassicMediaSupport,
    MultipleSupportedEvidenceSources,
    NoMappedAuthoritativeSource,
    UnknownPlatform,
    UnsupportedCanonicalPlatform,
}

impl CoverageRationale {
    pub fn label(self) -> &'static str {
        match self {
            Self::SupportedCartridgeDat => {
                "this canonical cartridge platform is covered by the supported No-Intro DAT importer"
            }
            Self::SupportedOpticalDat => {
                "this canonical optical-disc platform is covered by the supported Redump DAT importer"
            }
            Self::OfficialMameArcadeEvidence => {
                "Arcade identity is expected from official MAME evidence"
            }
            Self::OfficialScummVmDetectorEvidence => {
                "ScummVM identity is expected from the official detector evidence, not generic DAT matching"
            }
            Self::ExplicitTosecClassicMediaSupport => {
                "this platform has explicit classic-media TOSEC support in the current architecture"
            }
            Self::MultipleSupportedEvidenceSources => {
                "more than one supported evidence source is relevant; no source is silently discarded"
            }
            Self::NoMappedAuthoritativeSource => {
                "no authoritative ecosystem is currently mapped for this canonical platform"
            }
            Self::UnknownPlatform => "the platform is unknown, so no evidence source is inferred",
            Self::UnsupportedCanonicalPlatform => {
                "the platform is known to the registry but has no supported authoritative evidence mapping"
            }
        }
    }
}

/// One expected source and its authority role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedCoverageSource {
    pub source: ExpectedAuthoritativeSource,
    pub role: CoverageSourceRole,
    pub rationale: CoverageRationale,
}

/// Why a coverage expectation has no usable canonical platform mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnsupportedPlatformKind {
    Unknown,
    KnownButUnmapped,
}

/// The expectation for one platform, independent of whether its source is
/// registered, enabled, current, or successfully verified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum PlatformCoverageExpectation {
    ExpectedAuthoritativeSource {
        platform: String,
        source: ExpectedCoverageSource,
    },
    MultipleCandidateSources {
        platform: String,
        sources: Vec<ExpectedCoverageSource>,
        rationale: CoverageRationale,
    },
    NoKnownAuthoritativeSource {
        platform: String,
        rationale: CoverageRationale,
    },
    UnsupportedOrUnknown {
        platform: Option<String>,
        kind: UnsupportedPlatformKind,
        rationale: CoverageRationale,
    },
}

/// Lifecycle states that a later projection may calculate after joining an
/// expectation with registered-source state. This enum deliberately performs
/// no source lookup itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformCoverageState {
    Covered,
    ExpectedButMissing,
    ExpectedButInactive,
    ExpectedButStale,
    ExpectedButUnmanaged,
    NoExpectedSource,
    UnknownPlatform,
}

/// The canonical platforms whose reviewed authoritative ecosystem is TOSEC
/// alone. Every entry here must also have an exact TOSEC system projection in
/// [`super::tosec_system_projection`]; a test keeps the two in lockstep.
pub(crate) const TOSEC_PRIMARY_PLATFORMS: &[&str] = &[
    "Amiga",
    "Acorn Electron",
    "Amstrad CPC",
    "Apple II",
    "AtariST",
    "BBC Micro",
    "Commodore 128",
    "Commodore 64",
    "VIC-20",
    "ZX Spectrum",
];

fn tosec_primary_platform(platform: &str) -> bool {
    TOSEC_PRIMARY_PLATFORMS.contains(&platform)
}

/// Resolves a canonical platform id or an exact registry alias to its expected
/// authoritative evidence. Unknown text is never fuzzy-matched.
pub fn expected_authoritative_coverage(platform_hint: Option<&str>) -> PlatformCoverageExpectation {
    let Some(platform_hint) = platform_hint else {
        return PlatformCoverageExpectation::UnsupportedOrUnknown {
            platform: None,
            kind: UnsupportedPlatformKind::Unknown,
            rationale: CoverageRationale::UnknownPlatform,
        };
    };

    let Some(platform) = crate::canonical_platform_for_alias(platform_hint) else {
        return PlatformCoverageExpectation::UnsupportedOrUnknown {
            platform: Some(platform_hint.trim().to_string()),
            kind: UnsupportedPlatformKind::Unknown,
            rationale: CoverageRationale::UnknownPlatform,
        };
    };

    let source = |source: ExpectedAuthoritativeSource,
                  role: CoverageSourceRole,
                  rationale: CoverageRationale|
     -> ExpectedCoverageSource {
        ExpectedCoverageSource {
            source,
            role,
            rationale,
        }
    };

    let expected = match platform {
        // These are the cartridge families for which the current codebase has
        // No-Intro import/identity support. The list is intentionally explicit
        // instead of treating every platform with a ROM extension as No-Intro.
        "Atari2600" | "Atari5200" | "Atari7800" | "Atari Lynx" | "ColecoVision" | "GameGear"
        | "Game Boy" | "Game Boy Advance" | "Game Boy Color" | "MasterSystem" | "MegaDrive"
        | "N64" | "NES" | "Nintendo DS" | "PC Engine" | "SNES" | "Sega 32X" | "TurboGrafx-16"
        | "Virtual Boy" | "Watara Supervision" | "WonderSwan" | "WonderSwan Color" => {
            Some(PlatformCoverageExpectation::ExpectedAuthoritativeSource {
                platform: platform.to_string(),
                source: source(
                    ExpectedAuthoritativeSource::Dat(DatEcosystem::NoIntro),
                    CoverageSourceRole::Primary,
                    CoverageRationale::SupportedCartridgeDat,
                ),
            })
        }

        // These are the reviewed Redump game systems represented by the
        // current-main `RedumpGameSystem` table. Expectation is about the
        // authoritative ecosystem, not whether every system currently has a
        // managed downloader; acquisition mode remains a separate question
        // in current main.
        "PSX" | "PS2" | "PS3" | "PS4" | "PSP" | "Saturn" | "Dreamcast" | "Sega CD" | "GameCube"
        | "Wii" | "WiiU" | "Xbox" | "Xbox360" | "3DO" | "PC-FX" | "PC Engine CD" | "Neo Geo CD" => {
            Some(PlatformCoverageExpectation::ExpectedAuthoritativeSource {
                platform: platform.to_string(),
                source: source(
                    ExpectedAuthoritativeSource::Dat(DatEcosystem::Redump),
                    CoverageSourceRole::Primary,
                    CoverageRationale::SupportedOpticalDat,
                ),
            })
        }

        "Arcade" => Some(PlatformCoverageExpectation::MultipleCandidateSources {
            platform: platform.to_string(),
            sources: vec![
                source(
                    ExpectedAuthoritativeSource::Dat(DatEcosystem::MAMEArcade),
                    CoverageSourceRole::Primary,
                    CoverageRationale::OfficialMameArcadeEvidence,
                ),
                source(
                    ExpectedAuthoritativeSource::Dat(DatEcosystem::FBNeo),
                    CoverageSourceRole::Supplementary,
                    CoverageRationale::MultipleSupportedEvidenceSources,
                ),
            ],
            rationale: CoverageRationale::MultipleSupportedEvidenceSources,
        }),

        "ScummVM" => Some(PlatformCoverageExpectation::ExpectedAuthoritativeSource {
            platform: platform.to_string(),
            source: source(
                ExpectedAuthoritativeSource::OfficialScummVmDetector,
                CoverageSourceRole::Primary,
                CoverageRationale::OfficialScummVmDetectorEvidence,
            ),
        }),

        // Atari 8-bit keeps its existing No-Intro (cartridge) authority as the
        // primary source. Its disk/executable media (ATR/ATX/XFD/XEX) are
        // catalogued by TOSEC's "Atari 8bit" system, so TOSEC is retained as a
        // secondary source rather than replacing No-Intro.
        "Atari 8-bit" => Some(PlatformCoverageExpectation::MultipleCandidateSources {
            platform: platform.to_string(),
            sources: vec![
                source(
                    ExpectedAuthoritativeSource::Dat(DatEcosystem::NoIntro),
                    CoverageSourceRole::Primary,
                    CoverageRationale::SupportedCartridgeDat,
                ),
                source(
                    ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec),
                    CoverageSourceRole::Secondary,
                    CoverageRationale::ExplicitTosecClassicMediaSupport,
                ),
            ],
            rationale: CoverageRationale::MultipleSupportedEvidenceSources,
        }),

        // Classic computer platforms whose disk/tape/cartridge-file media are
        // catalogued by a TOSEC system group and verified through the shared
        // exact-hash DAT path (see `docs/research/DAT_D01_...`). This list is
        // reviewed one platform at a time: TOSEC DAT availability alone never
        // adds a platform here, and a platform not listed (MSX, DOS/PC,
        // Macintosh, Dragon/CoCo, Oric, Thomson, Japanese PCs ...) stays
        // `NoKnownAuthoritativeSource` until it has its own review. The
        // canonical platform must already be resolved from independent
        // evidence; a TOSEC name never establishes it (BBC Micro and Acorn
        // Electron stay separate entries for that reason).
        _ if tosec_primary_platform(platform) => {
            Some(PlatformCoverageExpectation::ExpectedAuthoritativeSource {
                platform: platform.to_string(),
                source: source(
                    ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec),
                    CoverageSourceRole::Primary,
                    CoverageRationale::ExplicitTosecClassicMediaSupport,
                ),
            })
        }

        _ => None,
    };

    expected.unwrap_or_else(|| PlatformCoverageExpectation::NoKnownAuthoritativeSource {
        platform: platform.to_string(),
        rationale: CoverageRationale::UnsupportedCanonicalPlatform,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_source(
        platform: Option<&str>,
        expected: ExpectedAuthoritativeSource,
    ) -> ExpectedCoverageSource {
        match expected_authoritative_coverage(platform) {
            PlatformCoverageExpectation::ExpectedAuthoritativeSource { source, .. } => {
                assert_eq!(source.source, expected);
                source
            }
            other => panic!("expected one source, got {other:?}"),
        }
    }

    #[test]
    fn representative_cartridge_platforms_expect_no_intro() {
        for platform in ["Game Boy", "Game Boy Advance", "NES", "SNES"] {
            single_source(
                Some(platform),
                ExpectedAuthoritativeSource::Dat(DatEcosystem::NoIntro),
            );
        }
    }

    #[test]
    fn sega_cartridge_aliases_expect_no_intro() {
        for platform in ["MasterSystem", "MegaDrive", "GameGear", "Sega 32X"] {
            single_source(
                Some(platform),
                ExpectedAuthoritativeSource::Dat(DatEcosystem::NoIntro),
            );
        }
    }

    #[test]
    fn all_current_redump_systems_expect_redump_authority() {
        for platform in [
            "PSX",
            "PS2",
            "PS3",
            "PS4",
            "PSP",
            "Saturn",
            "Dreamcast",
            "Sega CD",
            "GameCube",
            "Wii",
            "WiiU",
            "Xbox",
            "Xbox360",
            "3DO",
            "PC-FX",
            "PC Engine CD",
            "Neo Geo CD",
        ] {
            single_source(
                Some(platform),
                ExpectedAuthoritativeSource::Dat(DatEcosystem::Redump),
            );
        }
        assert!(matches!(
            expected_authoritative_coverage(Some("Philips CD-i")),
            PlatformCoverageExpectation::NoKnownAuthoritativeSource { .. }
        ));
    }

    #[test]
    fn official_mame_is_primary_and_fbneo_is_supplementary() {
        let PlatformCoverageExpectation::MultipleCandidateSources {
            platform,
            sources,
            rationale,
        } = expected_authoritative_coverage(Some("Arcade"))
        else {
            panic!("Arcade must retain both supported evidence sources");
        };
        assert_eq!(platform, "Arcade");
        assert_eq!(
            rationale,
            CoverageRationale::MultipleSupportedEvidenceSources
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].role, CoverageSourceRole::Primary);
        assert_eq!(
            sources[0].source,
            ExpectedAuthoritativeSource::Dat(DatEcosystem::MAMEArcade)
        );
        assert_eq!(sources[1].role, CoverageSourceRole::Supplementary);
        assert_eq!(
            sources[1].source,
            ExpectedAuthoritativeSource::Dat(DatEcosystem::FBNeo)
        );
    }

    #[test]
    fn scummvm_uses_official_detector_not_generic_dat_matching() {
        let source = single_source(
            Some("ScummVM"),
            ExpectedAuthoritativeSource::OfficialScummVmDetector,
        );
        assert_eq!(source.role, CoverageSourceRole::Primary);
    }

    #[test]
    fn amiga_is_the_explicit_tosec_classic_media_mapping() {
        single_source(
            Some("Amiga"),
            ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec),
        );
    }

    const REVIEWED_TOSEC_PLATFORMS: [&str; 10] = [
        "Amiga",
        "Acorn Electron",
        "Amstrad CPC",
        "Apple II",
        "AtariST",
        "BBC Micro",
        "Commodore 128",
        "Commodore 64",
        "VIC-20",
        "ZX Spectrum",
    ];

    #[test]
    fn reviewed_classic_computer_platforms_expect_tosec_as_primary() {
        for platform in REVIEWED_TOSEC_PLATFORMS {
            let source = single_source(
                Some(platform),
                ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec),
            );
            assert_eq!(source.role, CoverageSourceRole::Primary, "{platform}");
            assert_eq!(
                source.rationale,
                CoverageRationale::ExplicitTosecClassicMediaSupport
            );
        }
    }

    #[test]
    fn classic_platform_aliases_resolve_to_the_same_expectation() {
        for (alias, canonical) in [
            ("c64", "Commodore 64"),
            ("c128", "Commodore 128"),
            ("vic20", "VIC-20"),
            ("bbc", "BBC Micro"),
            ("bbcmicro", "BBC Micro"),
            ("elk", "Acorn Electron"),
            ("cpc", "Amstrad CPC"),
            ("atarist", "AtariST"),
            ("speccy", "ZX Spectrum"),
            ("apple2", "Apple II"),
            ("atari800", "Atari 8-bit"),
        ] {
            assert_eq!(
                expected_authoritative_coverage(Some(alias)),
                expected_authoritative_coverage(Some(canonical)),
                "{alias}"
            );
        }
    }

    #[test]
    fn bbc_micro_and_acorn_electron_are_separate_explicit_expectations() {
        let PlatformCoverageExpectation::ExpectedAuthoritativeSource { platform: bbc, .. } =
            expected_authoritative_coverage(Some("BBC Micro"))
        else {
            panic!("BBC Micro expects one source");
        };
        let PlatformCoverageExpectation::ExpectedAuthoritativeSource {
            platform: electron, ..
        } = expected_authoritative_coverage(Some("Acorn Electron"))
        else {
            panic!("Acorn Electron expects one source");
        };
        assert_eq!(bbc, "BBC Micro");
        assert_eq!(electron, "Acorn Electron");
    }

    #[test]
    fn atari_8bit_keeps_no_intro_primary_and_adds_tosec_secondary() {
        let PlatformCoverageExpectation::MultipleCandidateSources {
            platform,
            sources,
            rationale,
        } = expected_authoritative_coverage(Some("Atari 8-bit"))
        else {
            panic!("Atari 8-bit must retain No-Intro and add TOSEC");
        };
        assert_eq!(platform, "Atari 8-bit");
        assert_eq!(
            rationale,
            CoverageRationale::MultipleSupportedEvidenceSources
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].role, CoverageSourceRole::Primary);
        assert_eq!(
            sources[0].source,
            ExpectedAuthoritativeSource::Dat(DatEcosystem::NoIntro)
        );
        assert_eq!(sources[1].role, CoverageSourceRole::Secondary);
        assert_eq!(
            sources[1].source,
            ExpectedAuthoritativeSource::Dat(DatEcosystem::Tosec)
        );
    }

    #[test]
    fn no_intro_cartridge_platforms_keep_exactly_one_no_intro_source() {
        for platform in [
            "Atari2600",
            "Atari5200",
            "Atari7800",
            "Atari Lynx",
            "ColecoVision",
            "GameGear",
            "Game Boy",
            "Game Boy Advance",
            "Game Boy Color",
            "MasterSystem",
            "MegaDrive",
            "N64",
            "NES",
            "Nintendo DS",
            "PC Engine",
            "SNES",
            "Sega 32X",
            "TurboGrafx-16",
            "Virtual Boy",
            "Watara Supervision",
            "WonderSwan",
            "WonderSwan Color",
        ] {
            single_source(
                Some(platform),
                ExpectedAuthoritativeSource::Dat(DatEcosystem::NoIntro),
            );
        }
    }

    #[test]
    fn unreviewed_classic_platforms_stay_fail_closed_despite_tosec_existing() {
        // TOSEC publishes systems for several of these; availability is not
        // authority, so each needs its own review before it can be mapped.
        for platform in [
            "MSX",
            "MSX2",
            "DOS",
            "PC",
            "Macintosh",
            "Dragon / Tandy CoCo",
            "Oric",
            "Thomson MO",
            "Thomson TO",
            "Enterprise",
            "Acorn Archimedes",
            "NEC PC-8801",
            "PC-98",
            "NEC PC-9801",
            "Sharp X68000",
            "FM Towns",
            "Commodore CDTV",
            "AmigaCD32",
        ] {
            assert!(
                matches!(
                    expected_authoritative_coverage(Some(platform)),
                    PlatformCoverageExpectation::NoKnownAuthoritativeSource { .. }
                ),
                "{platform}"
            );
        }
    }

    #[test]
    fn tosec_names_and_unknown_text_never_become_a_platform() {
        for text in [
            "TOSEC",
            "Commodore C64 - Games - [D64] (TOSEC-v2025-02-16_CM)",
            "Commodore C64DTV",
            "Acorn",
        ] {
            assert!(
                matches!(
                    expected_authoritative_coverage(Some(text)),
                    PlatformCoverageExpectation::UnsupportedOrUnknown {
                        kind: UnsupportedPlatformKind::Unknown,
                        ..
                    }
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn classic_expectations_never_name_metadata_or_custom_sources_and_are_deterministic() {
        for platform in REVIEWED_TOSEC_PLATFORMS.iter().chain(&["Atari 8-bit"]) {
            let first = expected_authoritative_coverage(Some(platform));
            let serialized = serde_json::to_string(&first).unwrap();
            for forbidden in [
                "screen_scraper",
                "romm",
                "generic_logiqx",
                "generic_clr_mame_pro",
            ] {
                assert!(!serialized.contains(forbidden), "{platform}");
            }
            for _ in 0..16 {
                assert_eq!(expected_authoritative_coverage(Some(platform)), first);
            }
        }
    }

    #[test]
    fn aliases_resolve_to_the_same_canonical_expectation() {
        assert_eq!(
            expected_authoritative_coverage(Some("gb")),
            expected_authoritative_coverage(Some("Game Boy"))
        );
        assert_eq!(
            expected_authoritative_coverage(Some("ps2")),
            expected_authoritative_coverage(Some("PS2"))
        );
    }

    #[test]
    fn unknown_and_known_but_unmapped_platforms_fail_closed() {
        assert!(matches!(
            expected_authoritative_coverage(None),
            PlatformCoverageExpectation::UnsupportedOrUnknown {
                kind: UnsupportedPlatformKind::Unknown,
                ..
            }
        ));
        assert!(matches!(
            expected_authoritative_coverage(Some("not-a-platform")),
            PlatformCoverageExpectation::UnsupportedOrUnknown {
                kind: UnsupportedPlatformKind::Unknown,
                ..
            }
        ));
        assert!(matches!(
            expected_authoritative_coverage(Some("MSX")),
            PlatformCoverageExpectation::NoKnownAuthoritativeSource { .. }
        ));
    }

    #[test]
    fn metadata_and_local_custom_dat_ecosystems_are_never_expected_sources() {
        let serialized = serde_json::to_string(&expected_authoritative_coverage(Some("Arcade")))
            .expect("coverage expectation serializes");
        assert!(!serialized.contains("screen_scraper"));
        assert!(!serialized.contains("romm"));
        assert!(!serialized.contains("generic_logiqx"));
        assert!(!serialized.contains("generic_clr_mame_pro"));
    }

    #[test]
    fn repeated_resolution_is_deterministic() {
        let first = expected_authoritative_coverage(Some("Arcade"));
        for _ in 0..32 {
            assert_eq!(expected_authoritative_coverage(Some("Arcade")), first);
        }
    }
}
