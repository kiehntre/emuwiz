use super::*;
use CatalogueRecognition::*;

fn no_intro(name: &str) -> CatalogueRecognition {
    recognise_catalogue(Some(name), Some(DatEcosystem::NoIntro))
}

fn tosec(name: &str) -> CatalogueRecognition {
    recognise_catalogue(Some(name), Some(DatEcosystem::Tosec))
}

fn games(platform: &'static str) -> CatalogueRecognition {
    Games { platform }
}

fn extras(recognition: &CatalogueRecognition, platform: &str) -> String {
    match recognition {
        Extras { platform: p, what } if *p == platform => what.clone(),
        other => panic!("expected {platform} extras, got {other:?}"),
    }
}

#[test]
fn ordinary_no_intro_game_lists_are_recognised() {
    for (name, platform) in [
        ("Nintendo - Game Boy Color", "Game Boy Color"),
        ("Nintendo - Game Boy Advance", "Game Boy Advance"),
        ("Nintendo - Game Boy", "Game Boy"),
        ("Nintendo - Super Nintendo Entertainment System", "SNES"),
        ("Sega - Mega Drive - Genesis", "MegaDrive"),
        ("Sega - Master System - Mark III", "MasterSystem"),
        ("Commodore - Amiga", "Amiga"),
        ("Atari - Atari 2600", "Atari2600"),
        ("Acorn - Archimedes", "Acorn Archimedes"),
    ] {
        assert_eq!(no_intro(name), games(platform), "{name}");
    }
}

#[test]
fn dump_format_labels_keep_the_ordinary_list() {
    for (name, platform) in [
        ("Nintendo - Nintendo Entertainment System (Headered)", "NES"),
        (
            "Nintendo - Nintendo Entertainment System (Headerless)",
            "NES",
        ),
        ("Nintendo - Nintendo 64 (BigEndian)", "N64"),
        ("Atari - Atari Jaguar (J64)", "Atari Jaguar"),
        ("Commodore - Amiga (Flux)", "Amiga"),
    ] {
        assert_eq!(no_intro(name), games(platform), "{name}");
    }
}

#[test]
fn redump_style_names_are_recognised() {
    assert_eq!(
        recognise_catalogue(Some("Sony - PlayStation"), Some(DatEcosystem::Redump)),
        games("PSX")
    );
}

fn needs_review(recognition: &CatalogueRecognition, platform: &str) -> String {
    match recognition {
        NeedsReview { platform: p, why } if *p == platform => why.clone(),
        other => panic!("expected {platform} to need review, got {other:?}"),
    }
}

#[test]
fn non_redump_discs_are_recognised_but_always_need_a_person_to_review_them() {
    for (name, platform) in [
        ("Non-Redump - Sega - Dreamcast", "Dreamcast"),
        (
            "Non-Redump - NEC - PC Engine CD + TurboGrafx CD",
            "PC Engine CD",
        ),
        ("Non-Redump - NEC - PC-88 series", "NEC PC-8801"),
    ] {
        let recognition = no_intro(name);
        assert_eq!(
            needs_review(&recognition, platform),
            "collected outside Redump",
            "{name}"
        );
        // Never an ordinary game list, so never a safe match, but still the
        // system's catalogue for a person who chooses it.
        assert_eq!(recognition.safe_platform(), None, "{name}");
        assert!(recognition.is_for(platform), "{name}");
    }
    assert_eq!(
        no_intro("Non-Redump - Sega - Dreamcast").describe(),
        "Sega Dreamcast discs collected outside Redump"
    );
    // Extras stay visible in the reason.
    assert!(
        needs_review(
            &no_intro("Non-Redump - Sega - Dreamcast (Demos)"),
            "Dreamcast"
        )
        .contains("Demos"),
    );
    assert!(
        needs_review(
            &no_intro("Non-Redump - Sega - Dreamcast - Prototypes"),
            "Dreamcast"
        )
        .starts_with("collected outside Redump ("),
    );
}

#[test]
fn non_redump_catalogues_that_are_not_games_or_unknown_are_still_refused() {
    for name in [
        "Non-Redump - Audio CD",
        "Non-Redump - DVD-Video",
        "Non-Redump - BD-Video",
    ] {
        assert!(matches!(no_intro(name), NotGames { .. }), "{name}");
    }
    assert!(
        matches!(
            no_intro("Non-Redump - Acme - Hypothetical Console"),
            NotRecognised { .. }
        ),
        "an unknown system is never treated as a game list"
    );
    assert_eq!(
        no_intro("Non-Redump - Acme - Hypothetical Console").platform(),
        None
    );
}

#[test]
fn reviewed_names_the_alias_registry_does_not_spell() {
    assert_eq!(no_intro("Apple - II (WOZ)"), games("Apple II"));
    assert_eq!(no_intro("Apple - IIGS (A2R)"), games("Apple II"));
    assert_eq!(no_intro("Atari - 8-bit Family"), games("Atari 8-bit"));
    assert_eq!(no_intro("Sinclair - ZX Spectrum +3"), games("ZX Spectrum"));
}

#[test]
fn extras_name_the_right_system_but_are_never_the_ordinary_list() {
    let cases = [
        (
            "Nintendo - Game Boy Advance (e-Reader)",
            "Game Boy Advance",
            "e-Reader cards",
        ),
        (
            "Nintendo - Game Boy Advance (Multiboot)",
            "Game Boy Advance",
            "multiboot programs",
        ),
        (
            "Sony - PlayStation 3 (PSN) (DLC)",
            "PS3",
            "digital downloads, downloadable content",
        ),
        (
            "Nintendo - Nintendo GameCube (Memory Card)",
            "GameCube",
            "memory card data",
        ),
        (
            "Nintendo - Nintendo DS (Encrypted)",
            "Nintendo DS",
            "encrypted copies",
        ),
        (
            "Sega - Dreamcast (Development Kit Hard Drives)",
            "Dreamcast",
            "development material",
        ),
        ("Sony - PlayStation - BIOS Images", "PSX", "BIOS files"),
        (
            "Unofficial - Sony - PlayStation Vita (VPK)",
            "PlayStation Vita",
            "unofficial dumps, \u{201c}VPK\u{201d}",
        ),
    ];
    for (name, platform, what) in cases {
        let recognition = no_intro(name);
        assert_eq!(extras(&recognition, platform), what, "{name}");
        assert_eq!(recognition.safe_platform(), None, "{name}");
        assert!(recognition.is_for(platform));
    }
}

#[test]
fn add_on_systems_are_extras_of_their_parent() {
    assert_eq!(
        extras(
            &no_intro("Nintendo - Family Computer Disk System (FDS)"),
            "NES"
        ),
        "Famicom Disk System disks"
    );
    assert_eq!(
        extras(
            &no_intro("Nintendo - Nintendo DSi (Decrypted)"),
            "Nintendo DS"
        ),
        "DSi software, decrypted copies"
    );
    assert_eq!(
        extras(&no_intro("NEC - PC Engine SuperGrafx"), "PC Engine"),
        "SuperGrafx games"
    );
}

#[test]
fn an_unfamiliar_label_is_extras_not_the_ordinary_list() {
    let recognition = no_intro("Nintendo - Game Boy Color (Something New)");
    assert_eq!(
        extras(&recognition, "Game Boy Color"),
        "\u{201c}Something New\u{201d}"
    );
}

#[test]
fn non_game_catalogues_are_never_assigned() {
    for name in [
        "Source Code - Nintendo - Game Boy Color",
        "Source Code - Various",
        "Unofficial - Video Game OSTs",
        "Unofficial - Video Game Magazine Scans (Some Magazine)",
        "Unofficial - Super Mario Maker Courses",
        "Nintendo - amiibo",
        "Non-Redump - Audio CD",
        "CD-ROM",
    ] {
        let recognition = no_intro(name);
        assert!(
            matches!(recognition, NotGames { .. }),
            "{name}: {recognition:?}"
        );
        assert_eq!(recognition.platform(), None, "{name}");
    }
}

#[test]
fn multi_system_catalogues_are_never_assigned() {
    for ecosystem in [
        DatEcosystem::MAMEArcade,
        DatEcosystem::MAMESoftwareList,
        DatEcosystem::FBNeo,
    ] {
        assert_eq!(
            recognise_catalogue(Some("Nintendo - Game Boy"), Some(ecosystem)),
            SeveralSystems
        );
    }
    for name in ["Arcade - PC-based", "Project EGG", "Nintendo - Misc"] {
        assert_eq!(no_intro(name), SeveralSystems, "{name}");
    }
}

#[test]
fn a_header_naming_two_systems_is_ambiguous() {
    assert_eq!(
        no_intro("IBM - PC and Compatibles (Flux)"),
        Ambiguous {
            platforms: vec!["DOS", "PC"]
        }
    );
    let recognition = no_intro("Nintendo - Game Boy + Sega - Game Gear");
    assert!(matches!(recognition, Ambiguous { .. }), "{recognition:?}");
    assert_eq!(recognition.platform(), None);
}

#[test]
fn equivalent_names_for_one_machine_are_not_ambiguous() {
    assert_eq!(
        no_intro("NEC - PC Engine - TurboGrafx-16"),
        games("PC Engine")
    );
    assert_eq!(
        tosec("Sega Mark III & Master System - Games"),
        games("MasterSystem")
    );
}

#[test]
fn unsupported_systems_are_not_mapped_to_a_neighbour() {
    for name in [
        "Sony - PlayStation Mobile (PSN)",
        "Casio - Loopy",
        "Sega - SG-1000 - SC-3000",
        "Microsoft - Xbox One (Digital) (CDN)",
        "Totally Unknown Maker - Imaginary Console",
    ] {
        let recognition = no_intro(name);
        assert!(
            matches!(recognition, NotRecognised { .. }),
            "{name}: {recognition:?}"
        );
    }
}

#[test]
fn tosec_game_lists_are_recognised() {
    for (name, platform) in [
        ("Commodore Amiga - Games - [ADF]", "Amiga"),
        ("Sinclair ZX Spectrum - Games - [TAP]", "ZX Spectrum"),
        ("Nintendo Game Boy Advance - Games", "Game Boy Advance"),
        ("Atari Lynx - Games - [O]", "Atari Lynx"),
        ("Acorn BBC - Games - [SSD]", "BBC Micro"),
    ] {
        assert_eq!(tosec(name), games(platform), "{name}");
    }
}

#[test]
fn tosec_non_game_collections_are_extras_or_not_games() {
    assert_eq!(extras(&tosec("Commodore Amiga - Demos"), "Amiga"), "demos");
    assert_eq!(
        extras(
            &tosec("Acorn Archimedes - Compilations - Games - [JFD]"),
            "Acorn Archimedes"
        ),
        "compilations"
    );
    assert_eq!(
        extras(
            &tosec("Commodore Amiga - Games - Unofficial Addons & Patches"),
            "Amiga"
        ),
        "\u{201c}Unofficial Addons & Patches\u{201d}"
    );
    assert!(matches!(
        tosec("Commodore Amiga - Manuals"),
        NotGames { .. }
    ));
    assert!(matches!(
        tosec("Acorn Atom - Games - [UEF]"),
        NotRecognised { .. }
    ));
}

#[test]
fn a_missing_or_blank_header_name_is_never_guessed() {
    assert_eq!(
        recognise_catalogue(None, Some(DatEcosystem::NoIntro)),
        NoName
    );
    assert_eq!(recognise_catalogue(Some("   "), None), NoName);
}

#[test]
fn a_file_name_style_string_is_not_treated_as_a_header() {
    // A DAT file name carries a date and an extension; recognition is only
    // ever given header text, and this text names no system exactly.
    let recognition = no_intro("Nintendo - Game Boy Color (20260806-082610).dat");
    assert_eq!(recognition.safe_platform(), None, "{recognition:?}");
}

#[test]
fn descriptions_are_plain_english() {
    assert_eq!(
        no_intro("Nintendo - Game Boy Color").describe(),
        "Nintendo Game Boy Color games"
    );
    assert_eq!(
        no_intro("Source Code - Nintendo - Game Boy Color").describe(),
        "Not a game list (source code)"
    );
    assert!(
        no_intro("Nintendo - Game Boy Advance (e-Reader)")
            .describe()
            .ends_with("extras: e-Reader cards")
    );
}

#[test]
fn recognition_is_deterministic() {
    let name = "Sega - Mega Drive - Genesis";
    let first = no_intro(name);
    for _ in 0..10 {
        assert_eq!(no_intro(name), first);
    }
}

#[test]
fn an_unrecognised_name_segment_is_never_ignored() {
    // A trailing " - qualifier" that names no system could be a subset of
    // the system's games, so it is extras, never the ordinary list.
    for (name, platform, what) in [
        (
            "Nintendo - Nintendo 3DS - Digital",
            "Nintendo 3DS",
            "\u{201c}Digital\u{201d}",
        ),
        ("Sony - PlayStation 3 - Demos", "PS3", "demos"),
        (
            "Nintendo - Game Boy Color - Something New",
            "Game Boy Color",
            "\u{201c}Something New\u{201d}",
        ),
        (
            "Nintendo - Nintendo 3DS - Digital (CDN)",
            "Nintendo 3DS",
            "digital downloads, \u{201c}Digital\u{201d}",
        ),
    ] {
        let recognition = no_intro(name);
        assert_eq!(extras(&recognition, platform), what, "{name}");
        assert_eq!(recognition.safe_platform(), None, "{name}");
    }
}

#[test]
fn known_extras_qualifiers_stay_extras_and_spelled_names_stay_games() {
    assert_eq!(
        extras(&no_intro("Sony - PlayStation - BIOS Images"), "PSX"),
        "BIOS files"
    );
    assert_eq!(
        extras(
            &no_intro("Nintendo - Nintendo 3DS (Digital) (CDN)"),
            "Nintendo 3DS"
        ),
        "digital downloads"
    );
    // Every segment names the same machine, so these remain game lists.
    for (name, platform) in [
        ("Sega - Mega Drive - Genesis", "MegaDrive"),
        ("Sega - Master System - Mark III", "MasterSystem"),
        ("NEC - PC Engine - TurboGrafx-16", "PC Engine"),
    ] {
        assert_eq!(no_intro(name), games(platform), "{name}");
    }
}

#[test]
fn an_unknown_leading_word_is_never_ignored() {
    assert_eq!(
        extras(&no_intro("Hacks - Game Boy"), "Game Boy"),
        "\u{201c}Hacks\u{201d}"
    );
    let recognition = no_intro("Homebrew - Nintendo - Game Boy");
    assert_eq!(recognition.safe_platform(), None, "{recognition:?}");
    assert!(recognition.is_for("Game Boy"), "{recognition:?}");
    // Reviewed makers still lead ordinary lists.
    assert_eq!(no_intro("Nintendo - Game Boy"), games("Game Boy"));
}

#[test]
fn an_unknown_part_of_a_tosec_system_name_is_never_ignored() {
    let recognition = tosec("Nintendo Game Boy & Foo - Games");
    assert!(
        matches!(
            recognition,
            Extras {
                platform: "Game Boy",
                ..
            }
        ),
        "{recognition:?}"
    );
}

#[test]
fn reviewed_tosec_names_with_two_spellings_stay_game_lists() {
    for (name, platform) in [
        ("Nintendo Famicom & Entertainment System - Games", "NES"),
        ("Atari 2600 & VCS - Games", "Atari2600"),
        ("Sega Mega Drive & Genesis - Games", "MegaDrive"),
        ("NEC PC-Engine & TurboGrafx-16 - Games", "PC Engine"),
    ] {
        assert_eq!(tosec(name), games(platform), "{name}");
    }
}
