//! Catalogue admission regressions: a parsed prefix is never a valid catalogue.
use super::*;

fn parse(xml: &str) -> Result<ParseOutcome, ParseError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalogue.xml");
    std::fs::write(&path, xml).unwrap();
    parse_mame_listxml(&path, DatLimits::default())
}

#[test]
fn incomplete_and_multiple_roots_are_rejected() {
    for xml in [
        "<mame><machine name=\"good\"/>",
        "<mame><machine name=\"good\"/><machine name=\"unfinished\">",
        "<mame><machine name=\"good\"/></mame><mame/>",
        "<mame/><mame/>",
        "<mame><machine name=\"g\"><rom name=\"a\"/>",
        "<mame><machine name=\"g\"",
        "<mame><machine name=\"g\"></mame>",
        "",
    ] {
        assert!(
            matches!(parse(xml), Err(ParseError::MalformedXml { .. })),
            "{xml}"
        );
    }
    let error = parse("<mame><machine name=\"unfinished\">")
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("EOF") && error.contains("unfinished"),
        "{error}"
    );
}

#[test]
fn invalid_root_placement_and_outside_content_are_rejected() {
    for xml in [
        "<other><machine name=\"g\"/></other>",
        "<mame><mame/></mame>",
        "<mame><wrapper><machine name=\"g\"/></wrapper></mame>",
        "<mame><machine name=\"g\"><machine name=\"nested\"/></machine></mame>",
        "outside<mame/>",
        "<mame/>outside",
        "<mame/><![CDATA[outside]]>",
        "<mame/>&amp;",
        "<mame/><?xml version=\"1.0\"?>",
    ] {
        assert!(parse(xml).is_err(), "{xml}");
    }
}

#[test]
fn legitimate_document_preamble_empty_elements_and_unknown_extensions_survive() {
    let xml = r#"<?xml version="1.0"?>
<!DOCTYPE mame [<!ELEMENT mame ANY>]>
<!-- comment --><?tool version?>
<mame build="0.280"><machine name="g"><description>Example</description>
<future_extension><nested/></future_extension><rom name="r" sha1="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"/>
<disk name="d" sha1="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"/></machine><machine name="empty"/></mame>
<!-- tail -->"#;
    let outcome = parse(xml).unwrap();
    assert_eq!(outcome.dat.games.len(), 2);
    assert_eq!(
        outcome.dat.games[0].roms[0].sha1.as_deref(),
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
    );
    assert_eq!(
        outcome.dat.games[0].mame_listxml_schema_version,
        Some(MAME_LISTXML_PARSER_SCHEMA_VERSION)
    );
    assert!(parse("<mame/>").unwrap().dat.games.is_empty());
    assert!(parse("<mame></mame>").unwrap().dat.games.is_empty());
}

#[test]
fn malformed_supported_checksums_are_errors_with_member_context() {
    for (kind, algorithm) in [
        ("rom", "crc"),
        ("rom", "md5"),
        ("rom", "sha1"),
        ("disk", "sha1"),
    ] {
        for value in ["", " ", "malformed", "é", "gggggggg", "aa"] {
            let xml = format!(
                "<mame><machine name=\"game\"><{kind} name=\"member\" {algorithm}=\"{value}\"/></machine></mame>"
            );
            let error = parse(&xml).unwrap_err().to_string();
            for expected in ["game", "member", algorithm, "malformed"] {
                assert!(error.contains(expected), "{error}");
            }
        }
    }
}

#[test]
fn invalid_sha1_cannot_be_erased_by_matching_md5() {
    let xml = r#"<mame><machine name="g"><rom name="a.bin" size="11" md5="5eb63bbbe01eeed093cb22bb8f5acdc3" sha1="malformed"/></machine></mame>"#;
    assert!(parse(xml).unwrap_err().to_string().contains("sha1"));
}

#[test]
fn missing_checksums_and_preservation_statuses_remain_distinct_from_invalid() {
    let outcome = parse(r#"<mame><machine name="g"><rom name="missing"/><rom name="undumped" status="nodump"/><rom name="bad" status="baddump" crc="0D4A1185"/><disk name="disk" status="nodump"/></machine></mame>"#).unwrap();
    let game = &outcome.dat.games[0];
    assert_eq!(game.roms[0].sha1, None);
    assert_eq!(game.roms[1].status.as_deref(), Some("nodump"));
    assert_eq!(game.roms[2].crc32.as_deref(), Some("0d4a1185"));
    assert_eq!(game.disks[0].sha1, None);
    assert!(
        parse(
            r#"<mame><machine name="g"><rom name="n" status="nodump" sha1="bad"/></machine></mame>"#
        )
        .is_err()
    );
}

#[test]
fn malformed_and_duplicate_attributes_cannot_hide_checksum_declarations() {
    for attrs in [
        "sha1=\"bad\" sha1=\"also-bad\"",
        "name=\"other\" sha1=\"bad\"",
        "sha1=\"&unknown;\"",
        "sha1=unquoted",
    ] {
        let xml = format!("<mame><machine name=\"g\"><rom name=\"r\" {attrs}/></machine></mame>");
        assert!(parse(&xml).is_err(), "{xml}");
    }
}

#[test]
fn streaming_prefix_remains_provisional_until_document_validates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("partial.xml");
    std::fs::write(&path, "<mame><machine name=\"prefix\"/>").unwrap();
    let mut entries = Vec::new();
    let result =
        crate::dat::parsers::visit_dat_file_raw(&path, DatLimits::default(), &mut |game| {
            entries.push(game);
            Ok(())
        });
    assert_eq!(entries.len(), 1); // Progress is permitted, successful publication is not.
    assert!(result.is_err());
}

#[test]
fn depth_and_entry_limits_still_apply_to_empty_elements() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bounded.xml");
    std::fs::write(&path, "<mame><machine name=\"g\"/></mame>").unwrap();
    assert!(matches!(
        parse_mame_listxml(
            &path,
            DatLimits {
                max_xml_depth: 1,
                ..Default::default()
            }
        ),
        Err(ParseError::XmlDepthExceeded { .. })
    ));
    assert!(matches!(
        parse_mame_listxml(
            &path,
            DatLimits {
                max_entries: 0,
                ..Default::default()
            }
        ),
        Err(ParseError::EntryLimitExceeded { .. })
    ));
}
