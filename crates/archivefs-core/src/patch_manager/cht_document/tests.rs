//! Fixture-based tests for the full-fidelity `.cht` reader/writer.
//!
//! Every fixture below is shaped like real libretro-database cheat content
//! (`cheats = N` followed by `cheatN_desc`/`cheatN_code`/`cheatN_enable`),
//! not like a synthetic key-value blob.

use super::*;

/// A representative libretro-database file: quoted descriptions, quoted
/// Game Genie codes, explicit `_enable` on every entry, blank-line
/// separated blocks.
const REAL_WORLD_CHT: &str = "cheats = 3\n\
\n\
cheat0_desc = \"Infinite Health\"\n\
cheat0_code = \"NNVOSPVG\"\n\
cheat0_enable = false\n\
\n\
cheat1_desc = \"Infinite Lives\"\n\
cheat1_code = \"SZNKZOVK+SZVKAOVK\"\n\
cheat1_enable = false\n\
\n\
cheat2_desc = \"Start with 9 Bombs\"\n\
cheat2_code = \"PANKGOLA\"\n\
cheat2_enable = true\n";

/// The other common shape: no quotes at all, `cheats` last, extra
/// per-entry fields RetroArch itself writes for RAM-search cheats.
const UNQUOTED_CHT: &str = "cheat0_desc = Moon Jump\n\
cheat0_code = 8003F0A0 0009\n\
cheat0_enable = true\n\
cheat0_handler = 1\n\
cheat0_memory_search_size = 3\n\
cheats = 1\n";

fn parse(text: &str) -> ChtDocument {
    parse_cht_text(text).expect("fixture parses")
}

#[test]
fn parses_a_representative_libretro_cheat_file() {
    let document = parse(REAL_WORLD_CHT);
    assert_eq!(document.declared_count, Some(3));
    assert_eq!(document.entries.len(), 3);
    assert_eq!(document.selectable_count(), 3);
    assert!(
        document.warnings.is_empty(),
        "clean fixture produced {:?}",
        document.warnings
    );

    let first = &document.entries[0];
    assert_eq!(first.index, 0);
    assert_eq!(first.description.as_deref(), Some("Infinite Health"));
    assert_eq!(first.code.as_deref(), Some("NNVOSPVG"));
    assert!(!first.enabled_by_default);
    assert!(first.is_selectable());

    let third = &document.entries[2];
    assert!(third.enabled_by_default, "cheat2_enable = true is retained");
    assert_eq!(third.code.as_deref(), Some("PANKGOLA"));
}

#[test]
fn parses_unquoted_values_and_preserves_unknown_entry_fields() {
    let document = parse(UNQUOTED_CHT);
    assert_eq!(document.entries.len(), 1);
    let entry = &document.entries[0];
    assert_eq!(entry.description.as_deref(), Some("Moon Jump"));
    assert_eq!(entry.code.as_deref(), Some("8003F0A0 0009"));
    assert!(entry.enabled_by_default);
    assert_eq!(
        entry.extra_fields,
        vec![
            ("handler".to_string(), "1".to_string()),
            ("memory_search_size".to_string(), "3".to_string()),
        ],
        "non-desc/code/enable fields are preserved in first-seen order"
    );
}

#[test]
fn preserves_leading_comments_and_global_keys() {
    let text = "# Nintendo - NES\n# Source: example database\ncheats = 1\ncheat_delay = 4\n\
                cheat0_desc = \"A\"\ncheat0_code = \"B\"\n";
    let document = parse(text);
    assert_eq!(
        document.preserved_comments,
        vec![
            "Nintendo - NES".to_string(),
            "Source: example database".to_string()
        ]
    );
    assert_eq!(
        document.global_fields,
        vec![("cheat_delay".to_string(), "4".to_string())],
        "a non-cheatN key is preserved rather than dropped or mis-parsed"
    );
}

#[test]
fn blank_lines_and_trailing_comments_never_warn() {
    let text = "\n\n# header\ncheats = 1\n\n\ncheat0_desc = \"A\"\n\ncheat0_code = \"B\"\n\n";
    let document = parse(text);
    assert!(document.warnings.is_empty(), "{:?}", document.warnings);
    assert_eq!(document.entries.len(), 1);
}

#[test]
fn quoted_values_containing_escaped_quotes_are_rejected_without_rewriting_them() {
    let text = "cheats = 1\ncheat0_desc = \"Say \\\"hello\\\"\"\ncheat0_code = \"ABCD\"\n";
    let document = parse(text);
    let entry = &document.entries[0];
    assert_eq!(entry.description.as_deref(), Some(r#"Say \"hello\""#));
    assert!(
        entry
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::QuoteNormalized),
        "an interior quote is reported, because rendering substitutes it"
    );
    assert!(
        !entry.is_selectable(),
        "a value that would be changed during rendering is never installed"
    );
}

#[test]
fn malformed_entries_at_every_position_are_skipped_with_their_source_context() {
    let text = "cheats = 5\n\
                cheat0_desc = \"missing first code\"\n\
                cheat1_desc = \"first valid\"\ncheat1_code = \"A\"\n\
                cheat2_desc = \"missing middle code\"\n\
                cheat3_desc = \"second valid\"\ncheat3_code = \"B\"\n\
                cheat4_desc = \"missing final code\"\n";
    let document = parse(text);
    assert_eq!(document.selectable_count(), 2);
    for index in [0, 2, 4] {
        let entry = document
            .entry(index)
            .expect("malformed entry retained for review");
        assert!(!entry.is_selectable());
        let warning = entry
            .warnings
            .iter()
            .find(|warning| warning.kind == ChtEntryWarningKind::MissingCode)
            .expect("missing-code reason is reported");
        assert!(warning.line.is_some());
        assert!(
            warning
                .raw_source
                .as_deref()
                .is_some_and(|raw| raw.contains(&format!("cheat{index}_desc")))
        );
    }
    let rendered = render_cht_file(&install_entries(&document, &[0, 1, 2, 3, 4]), &[]);
    assert!(rendered.contains("first valid"));
    assert!(rendered.contains("second valid"));
    assert!(!rendered.contains("missing first code"));
    assert!(!rendered.contains("missing middle code"));
    assert!(!rendered.contains("missing final code"));
}

#[test]
fn malformed_lines_are_reported_without_losing_the_rest_of_the_file() {
    let text = "cheats = 2\nthis line has no separator\ncheat0_desc = \"A\"\ncheat0_code = \"B\"\n\
                cheat1_desc = \"C\"\ncheat1_code = \"D\"\n";
    let document = parse(text);
    assert_eq!(document.entries.len(), 2, "both good entries survive");
    let warning = document
        .warnings
        .iter()
        .find(|warning| warning.kind == ChtDocumentWarningKind::MalformedLine)
        .expect("malformed line reported");
    assert_eq!(warning.line, Some(2), "the exact source line is reported");
}

#[test]
fn a_malformed_entry_index_is_rejected_safely() {
    let text = "cheats = 1\ncheatX_desc = \"A\"\ncheat0_desc = \"B\"\ncheat0_code = \"C\"\n";
    let document = parse(text);
    assert_eq!(document.entries.len(), 1);
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::MalformedEntryIndex)
    );
}

#[test]
fn an_out_of_range_entry_index_is_rejected_rather_than_allocated() {
    let text = "cheats = 1\ncheat4294967296_desc = \"A\"\ncheat0_code = \"B\"\n";
    let document = parse(text);
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::EntryIndexOutOfRange)
    );
    assert!(
        document.entries.iter().all(|entry| entry.index == 0),
        "no entry is created for the out-of-range index"
    );
}

#[test]
fn an_entry_with_no_code_is_reported_and_is_not_selectable() {
    let text = "cheats = 2\ncheat0_desc = \"A\"\ncheat0_code = \"B\"\ncheat1_desc = \"No code\"\n";
    let document = parse(text);
    let broken = document.entry(1).expect("entry 1 exists");
    assert!(!broken.is_selectable());
    assert!(
        broken
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::MissingCode)
    );
    assert_eq!(document.selectable_count(), 1);
}

#[test]
fn an_empty_code_is_blocking() {
    let text = "cheats = 1\ncheat0_desc = \"A\"\ncheat0_code = \"\"\n";
    let document = parse(text);
    let entry = &document.entries[0];
    assert!(!entry.is_selectable());
    assert!(
        entry
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::EmptyCode)
    );
}

#[test]
fn a_missing_description_is_non_blocking_and_gets_a_stable_label() {
    let text = "cheats = 1\ncheat0_code = \"ABCD\"\n";
    let document = parse(text);
    let entry = &document.entries[0];
    assert!(entry.is_selectable(), "a code with no name is still usable");
    assert_eq!(entry.effective_description(), "Cheat 0");
    assert!(
        entry
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::MissingDescription)
    );
}

#[test]
fn a_duplicate_field_keeps_the_first_value_and_reports_the_conflict() {
    let text =
        "cheats = 1\ncheat0_desc = \"First\"\ncheat0_desc = \"Second\"\ncheat0_code = \"A\"\n";
    let document = parse(text);
    let entry = &document.entries[0];
    assert_eq!(entry.description.as_deref(), Some("First"));
    assert!(
        entry
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::ConflictingDuplicate)
    );
    assert!(!entry.is_selectable());
}

#[test]
fn an_unparsable_enable_value_defaults_to_disabled_and_reports_it() {
    let text = "cheats = 1\ncheat0_desc = \"A\"\ncheat0_code = \"B\"\ncheat0_enable = maybe\n";
    let document = parse(text);
    let entry = &document.entries[0];
    assert!(!entry.enabled_by_default);
    assert!(
        !entry.is_selectable(),
        "an unknown default is unsafe to install"
    );
    assert!(
        entry
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::UnparsableEnableValue)
    );
}

#[test]
fn a_declared_count_mismatch_is_reported_but_the_file_stays_usable() {
    let text = "cheats = 9\ncheat0_desc = \"A\"\ncheat0_code = \"B\"\n";
    let document = parse(text);
    assert_eq!(document.entries.len(), 1);
    assert_eq!(document.selectable_count(), 1);
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::DeclaredCountMismatch)
    );
}

#[test]
fn non_contiguous_indexes_are_reported_and_never_repaired_in_place() {
    let text = "cheats = 2\ncheat0_desc = \"A\"\ncheat0_code = \"B\"\ncheat7_desc = \"C\"\n\
                cheat7_code = \"D\"\n";
    let document = parse(text);
    assert_eq!(
        document
            .entries
            .iter()
            .map(|entry| entry.index)
            .collect::<Vec<_>>(),
        vec![0, 7],
        "source indexes are preserved exactly as declared"
    );
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::NonContiguousIndexes)
    );
}

#[test]
fn entries_stay_in_catalogue_order() {
    let text = "cheats = 3\ncheat2_desc = \"Third\"\ncheat2_code = \"C\"\n\
                cheat0_desc = \"First\"\ncheat0_code = \"A\"\n\
                cheat1_desc = \"Second\"\ncheat1_code = \"B\"\n";
    let document = parse(text);
    assert_eq!(
        document
            .entries
            .iter()
            .map(ChtEntry::effective_description)
            .collect::<Vec<_>>(),
        vec![
            "First".to_string(),
            "Second".to_string(),
            "Third".to_string()
        ],
        "declared index order, not file order, is the catalogue order"
    );
}

#[test]
fn utf16_content_is_reported_as_unsupported_encoding() {
    let bytes = [0xFF, 0xFE, b'c', 0x00, b'h', 0x00];
    let error = parse_cht_bytes(&bytes).expect_err("UTF-16 is rejected");
    assert_eq!(error.kind, ChtParseErrorKind::UnsupportedUtf16Encoding);
}

#[test]
fn invalid_utf8_is_reported_rather_than_lossily_decoded() {
    let mut bytes = b"cheats = 1\ncheat0_desc = \"".to_vec();
    bytes.push(0xFF);
    bytes.extend_from_slice(b"\"\n");
    let error = parse_cht_bytes(&bytes).expect_err("invalid UTF-8 is rejected");
    assert_eq!(error.kind, ChtParseErrorKind::UnsupportedEncoding);
}

#[test]
fn a_utf8_bom_is_tolerated() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(REAL_WORLD_CHT.as_bytes());
    let document = parse_cht_bytes(&bytes).expect("a UTF-8 BOM does not make the file unreadable");
    assert_eq!(document.entries.len(), 3);
}

#[test]
fn a_non_cheat_file_is_rejected_rather_than_returning_an_empty_document() {
    let error = parse_cht_text("hello = world\n").expect_err("not a cheat file");
    assert_eq!(error.kind, ChtParseErrorKind::NotACheatFile);
}

#[test]
fn arbitrary_binary_and_text_input_never_panics() {
    let inputs: [&[u8]; 8] = [
        b"",
        b"=",
        b"cheat",
        b"cheat_",
        b"cheat0_",
        b"cheats =",
        b"cheat99999999999999999999_desc = x",
        b"\xff\xfe\x00\x00",
    ];
    for input in inputs {
        // Only the return value matters: neither branch may panic.
        let _ = parse_cht_bytes(input);
    }
    for size in [0usize, 1, 7, 64] {
        let noise: Vec<u8> = (0..size).map(|value| (value % 251) as u8).collect();
        let _ = parse_cht_bytes(&noise);
    }
}

// -----------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------

fn install_entries(document: &ChtDocument, indexes: &[u32]) -> Vec<ChtInstallEntry> {
    indexes
        .iter()
        .filter_map(|index| document.entry(*index))
        .filter_map(|entry| ChtInstallEntry::from_entry(entry, entry.enabled_by_default))
        .collect()
}

#[test]
fn rendering_a_subset_renumbers_contiguously_from_zero() {
    let document = parse(REAL_WORLD_CHT);
    let rendered = render_cht_file(&install_entries(&document, &[0, 2]), &[]);
    assert_eq!(
        rendered,
        "cheats = 2\n\
\n\
cheat0_desc = \"Infinite Health\"\n\
cheat0_code = \"NNVOSPVG\"\n\
cheat0_enable = false\n\
\n\
cheat1_desc = \"Start with 9 Bombs\"\n\
cheat1_code = \"PANKGOLA\"\n\
cheat1_enable = true\n"
    );
}

#[test]
fn rendering_never_includes_an_unselected_entry() {
    let document = parse(REAL_WORLD_CHT);
    let rendered = render_cht_file(&install_entries(&document, &[1]), &[]);
    assert!(rendered.contains("Infinite Lives"));
    assert!(!rendered.contains("Infinite Health"));
    assert!(!rendered.contains("Start with 9 Bombs"));
    assert!(rendered.starts_with("cheats = 1\n"));
}

#[test]
fn rendering_is_deterministic() {
    let document = parse(REAL_WORLD_CHT);
    let first = render_cht_file(
        &install_entries(&document, &[0, 1, 2]),
        &["EmuWiz".to_string()],
    );
    let second = render_cht_file(
        &install_entries(&document, &[0, 1, 2]),
        &["EmuWiz".to_string()],
    );
    assert_eq!(first, second);
}

#[test]
fn rendering_always_ends_with_exactly_one_newline() {
    let document = parse(REAL_WORLD_CHT);
    for selection in [&[][..], &[0][..], &[0, 1, 2][..]] {
        let rendered = render_cht_file(&install_entries(&document, selection), &[]);
        assert!(rendered.ends_with('\n'), "{rendered:?}");
        assert!(!rendered.ends_with("\n\n"), "{rendered:?}");
    }
}

#[test]
fn an_empty_selection_still_renders_a_valid_header() {
    assert_eq!(render_cht_file(&[], &[]), "cheats = 0\n");
}

#[test]
fn the_enabled_flag_is_independent_of_the_source_default() {
    let document = parse(REAL_WORLD_CHT);
    let entry = document.entry(2).expect("entry 2");
    assert!(entry.enabled_by_default);
    let rendered = render_cht_file(
        &[ChtInstallEntry::from_entry(entry, false).expect("selectable")],
        &[],
    );
    assert!(
        rendered.contains("cheat0_enable = false"),
        "'included in the file' and 'enabled now' are separate decisions"
    );
}

#[test]
fn preserved_entry_fields_are_written_back_under_the_new_index() {
    let document = parse(UNQUOTED_CHT);
    let rendered = render_cht_file(&install_entries(&document, &[0]), &[]);
    assert!(rendered.contains("cheat0_handler = \"1\""));
    assert!(rendered.contains("cheat0_memory_search_size = \"3\""));
}

#[test]
fn header_comments_are_written_before_the_header_and_stripped_of_control_bytes() {
    let document = parse(REAL_WORLD_CHT);
    let rendered = render_cht_file(
        &install_entries(&document, &[0]),
        &["Installed by EmuWiz\u{7}".to_string()],
    );
    assert!(rendered.starts_with("# Installed by EmuWiz\n\ncheats = 1\n"));
}

#[test]
fn an_unselectable_entry_can_never_be_turned_into_an_install_entry() {
    let document = parse("cheats = 1\ncheat0_desc = \"No code\"\n");
    let entry = &document.entries[0];
    assert!(ChtInstallEntry::from_entry(entry, true).is_none());
}

#[test]
fn interior_quotes_are_not_silently_normalized_into_an_installed_entry() {
    let document = parse("cheats = 1\ncheat0_desc = \"Say \\\"hi\\\"\"\ncheat0_code = \"AB\"\n");
    let rendered = render_cht_file(&install_entries(&document, &[0]), &[]);
    assert_eq!(rendered, "cheats = 0\n");
}

#[test]
fn a_rendered_file_parses_back_to_the_same_selection() {
    let document = parse(REAL_WORLD_CHT);
    let rendered = render_cht_file(&install_entries(&document, &[0, 2]), &[]);
    let reparsed = parse(&rendered);
    assert_eq!(reparsed.declared_count, Some(2));
    assert!(reparsed.warnings.is_empty(), "{:?}", reparsed.warnings);
    assert_eq!(
        reparsed
            .entries
            .iter()
            .map(ChtEntry::effective_description)
            .collect::<Vec<_>>(),
        vec![
            "Infinite Health".to_string(),
            "Start with 9 Bombs".to_string()
        ]
    );
    assert_eq!(reparsed.entries[1].code.as_deref(), Some("PANKGOLA"));
}

#[test]
fn minimal_valid_and_missing_count_preserve_actual_evidence() {
    for (header, expected_count) in [("cheats = 1\n", Some(1)), ("", None)] {
        let document = parse(&format!(
            "{header}cheat0_desc = Lives\ncheat0_code = VALID\n"
        ));
        assert_eq!(document.declared_count, expected_count);
        assert_eq!(document.selectable_count(), 1);
        assert_eq!(
            document
                .warnings
                .iter()
                .any(|warning| warning.kind == ChtDocumentWarningKind::MissingDeclaredCount),
            header.is_empty()
        );
    }
}

#[test]
fn declared_counts_are_evidence_never_allocation_instructions() {
    for (count, kind) in [
        ("4294967295", ChtDocumentWarningKind::OversizedDeclaredCount),
        (
            "99999999999999999999",
            ChtDocumentWarningKind::MalformedDeclaredCount,
        ),
        ("-1", ChtDocumentWarningKind::MalformedDeclaredCount),
        ("no", ChtDocumentWarningKind::MalformedDeclaredCount),
        ("", ChtDocumentWarningKind::MalformedDeclaredCount),
        ("\"1", ChtDocumentWarningKind::MalformedDeclaredCount),
    ] {
        let document = parse(&format!(
            "cheats = {count}\ncheat0_desc = Lives\ncheat0_code = VALID\n"
        ));
        assert_eq!(document.entries.len(), 1, "{count}");
        assert_eq!(document.selectable_count(), 1, "{count}");
        assert!(
            document.warnings.iter().any(|warning| warning.kind == kind),
            "{count}"
        );
    }
    // Even a malformed count-only file retains its typed diagnostic.
    assert!(
        parse("cheats = -1")
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::MalformedDeclaredCount)
    );
}

#[test]
fn sparse_high_indices_are_sorted_and_count_range_disagreement_is_visible() {
    let document = parse(
        "cheats = 3\ncheat4_code = C\ncheat0_code = A\ncheat2_code = B\ncheat4294967295_code = D\n",
    );
    assert_eq!(
        document
            .entries
            .iter()
            .map(|entry| entry.index)
            .collect::<Vec<_>>(),
        vec![0, 2, 4, u32::MAX]
    );
    assert_eq!(document.selectable_count(), 4);
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::DeclaredCountMismatch)
    );
    let document = parse("cheats = 2\ncheat0_code = A\ncheat4_code = B\n");
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::DeclaredCountMismatch)
    );
}

#[test]
fn duplicates_are_observable_and_conflicts_block_only_the_affected_index() {
    for field in ["desc", "code", "enable", "handler", "future_field"] {
        let value = match field {
            "enable" => "true",
            "handler" => "1",
            _ => "VALID",
        };
        for (later, kind, selectable) in [
            (value, ChtEntryWarningKind::DuplicateField, true),
            (
                "DIFFERENT",
                ChtEntryWarningKind::ConflictingDuplicate,
                false,
            ),
        ] {
            let other_fields = match field {
                "desc" => "cheat1_code = VALID\n",
                "code" => "cheat1_desc = Lives\n",
                _ => "cheat1_desc = Lives\ncheat1_code = VALID\n",
            };
            let text = format!(
                "cheats = 3\ncheat0_code = A\n{other_fields}cheat1_{field} = {value}\ncheat1_{field} = {later}\ncheat2_code = B\n"
            );
            let document = parse(&text);
            let entry = document.entry(1).unwrap();
            assert_eq!(entry.is_selectable(), selectable, "{field} {later}");
            let warning = entry
                .warnings
                .iter()
                .find(|warning| warning.kind == kind)
                .unwrap();
            assert!(warning.line.is_some());
            assert!(warning.raw_source.as_ref().unwrap().contains(later));
            assert!(document.entry(0).unwrap().is_selectable());
            assert!(document.entry(2).unwrap().is_selectable());
        }
    }
}

#[test]
fn duplicate_index_blocks_conflicting_entry_blocks_instead_of_merging_them() {
    let document = parse(
        "cheats = 1\ncheat0_desc = First\ncheat0_code = AAA\ncheat00_desc = Second\ncheat00_code = BBB\n",
    );
    assert_eq!(document.entries.len(), 1);
    assert_eq!(document.entries[0].code.as_deref(), Some("AAA"));
    assert_eq!(document.entries[0].description.as_deref(), Some("First"));
    assert!(!document.entries[0].is_selectable());
    assert_eq!(
        document.entries[0]
            .warnings
            .iter()
            .filter(|warning| warning.kind == ChtEntryWarningKind::ConflictingDuplicate)
            .count(),
        2
    );
}

#[test]
fn duplicate_count_and_global_keys_keep_first_evidence_with_typed_diagnostics() {
    for (later, kind) in [
        ("1", ChtDocumentWarningKind::DuplicateField),
        ("2", ChtDocumentWarningKind::ConflictingDuplicate),
    ] {
        let document = parse(&format!(
            "cheats = 1\nCHEATS = {later}\ncustom = 1\ncustom = {later}\ncheat0_code = A\n"
        ));
        assert_eq!(document.declared_count, Some(1));
        assert_eq!(
            document.global_fields,
            vec![("custom".to_string(), "1".to_string())]
        );
        assert_eq!(
            document
                .warnings
                .iter()
                .filter(|warning| warning.kind == kind)
                .count(),
            2
        );
        assert_eq!(document.selectable_count(), 1);
    }
}

#[test]
fn unknown_fields_remain_preserved_and_nonblocking() {
    let document = parse(
        "cheats = 1\ncustom_tool = safe\ncheat0_desc = A\ncheat0_code = B\ncheat0_future_setting = xyz\n",
    );
    let entry = &document.entries[0];
    assert!(entry.is_selectable());
    assert_eq!(
        entry.extra_fields,
        vec![("future_setting".to_string(), "xyz".to_string())]
    );
    assert!(
        entry
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::UnsupportedField)
    );
}

#[test]
fn malformed_scalar_fields_block_entries_without_inventing_values() {
    for (field, value) in [
        ("handler", "-1"),
        ("value", "4294967296"),
        ("address", "0xZZ"),
        ("repeat_count", "maybe"),
        ("memory_search_size", ""),
        ("big_endian", "yes"),
        ("bad-field", "1"),
    ] {
        let document = parse(&format!(
            "cheats = 1\ncheat0_code = VALID\ncheat0_{field} = {value}\n"
        ));
        let entry = &document.entries[0];
        assert!(!entry.is_selectable(), "{field} {value}");
        assert!(
            entry
                .warnings
                .iter()
                .any(|warning| warning.kind == ChtEntryWarningKind::InvalidFieldValue)
        );
        assert_eq!(entry.extra_fields[0].1, value);
    }
    for value in ["0", "4294967295", "0xFF"] {
        let document = parse(&format!("cheat0_code = VALID\ncheat0_value = {value}\n"));
        assert!(document.entries[0].is_selectable());
    }
}

#[test]
fn malformed_entry_fixture_set_keeps_both_valid_neighbors() {
    for broken in [
        "cheat1_desc = Broken\n",
        "cheat1_code =\n",
        "cheat1_code = \"\"\n",
        "cheat1_code = \"unterminated",
        "cheat1_code = VALID\ncheat1_enable = maybe",
        "cheat1_code = VALID\ncheat1_code",
        "cheat1_code = VALID\ncheat1_ = x",
        "cheat1_code = +",
        "cheat1_code = A++B",
        "cheat1_code = \" \"",
        "cheat1_code = \u{fffd}",
        "cheat1_code = X\0Y",
    ] {
        let text = format!(
            "cheats = 3\ncheat0_desc = Lives\ncheat0_code = A\n{broken}\ncheat2_desc = Levels\ncheat2_code = B\n"
        );
        let document = parse(&text);
        assert_eq!(document.entries.len(), 3, "{broken:?}");
        assert_eq!(document.selectable_count(), 2, "{broken:?}");
        assert!(!document.entry(1).unwrap().is_selectable(), "{broken:?}");
        assert!(!document.entry(1).unwrap().warnings.is_empty());
    }
    // End-of-file without a newline is valid when the value is closed.
    assert_eq!(parse("cheat0_code = \"A\"").selectable_count(), 1);
    let truncated = parse("cheat0_code = A\ncheat1_code = \"B");
    assert_eq!(truncated.selectable_count(), 1);
    assert!(
        truncated
            .entry(1)
            .unwrap()
            .warnings
            .iter()
            .any(
                |warning| warning.kind == ChtEntryWarningKind::TruncatedValue
                    && warning.line == Some(2)
            )
    );
}

#[test]
fn bounded_values_lines_and_file_sizes_are_enforced_at_both_parser_entrypoints() {
    for size in [MAX_CHT_FIELD_BYTES + 1, MAX_CHT_LINE_BYTES + 1] {
        let text = format!(
            "cheats = 2\ncheat0_code = {}\ncheat1_code = A\n",
            "é".repeat(size)
        );
        let document = parse(&text);
        assert_eq!(document.selectable_count(), 1);
        let entry = document.entry(0).unwrap();
        assert!(entry.code.as_ref().unwrap().len() <= MAX_CHT_FIELD_BYTES);
        assert!(
            entry
                .warnings
                .iter()
                .any(|warning| warning.kind == ChtEntryWarningKind::OversizedField)
        );
        for warning in &entry.warnings {
            assert!(
                warning
                    .raw_source
                    .as_ref()
                    .is_none_or(|raw| raw.len() <= MAX_CHT_LINE_BYTES)
            );
        }
    }
    // An '=' past the line bound cannot conceal an unsafe duplicate assignment.
    let text = format!(
        "cheat0_code = A\ncheat0_code{}=B",
        " ".repeat(MAX_CHT_LINE_BYTES)
    );
    assert_eq!(parse(&text).selectable_count(), 0);
    let oversized = "x".repeat(MAX_CHT_FILE_BYTES + 1);
    assert_eq!(
        parse_cht_text(&oversized).unwrap_err().kind,
        ChtParseErrorKind::OversizedInput
    );
    assert_eq!(
        parse_cht_bytes(oversized.as_bytes()).unwrap_err().kind,
        ChtParseErrorKind::OversizedInput
    );
}

#[test]
fn code_component_field_entry_and_warning_limits_cannot_hide_bad_evidence() {
    let document = parse(&format!(
        "cheat0_code = {}",
        vec!["A"; MAX_CHT_CODE_LINES + 1].join("+")
    ));
    assert_eq!(document.selectable_count(), 0);
    assert!(
        document.entries[0]
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::MalformedCode)
    );
    let mut text = String::from("cheat0_code = A\n");
    for i in 0..MAX_CHT_EXTRA_FIELDS_PER_ENTRY + 1 {
        text.push_str(&format!("cheat0_future_{i} = x\n"));
    }
    let document = parse(&text);
    assert_eq!(document.selectable_count(), 0);
    assert_eq!(
        document.entries[0].extra_fields.len(),
        MAX_CHT_EXTRA_FIELDS_PER_ENTRY
    );
    assert!(document.entries[0].warnings.len() <= MAX_CHT_ENTRY_WARNINGS);
    let mut text = String::from("cheat0_code = A\n");
    for _ in 0..MAX_CHT_ENTRY_WARNINGS + 20 {
        text.push_str("cheat0_code = A\n");
    }
    text.push_str("cheat0_code = B\n");
    let document = parse(&text);
    assert_eq!(document.entries[0].warnings.len(), MAX_CHT_ENTRY_WARNINGS);
    assert_eq!(document.selectable_count(), 0);
    assert!(
        document.entries[0]
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::LimitReached)
    );
    let mut text = String::new();
    for i in 0..MAX_CHT_ENTRIES + 2 {
        text.push_str(&format!("cheat{i}_code = A\n"));
    }
    // Existing entries are still parsed after reaching the distinct-entry cap.
    text.push_str("cheat0_enable = maybe\n");
    let document = parse(&text);
    assert_eq!(document.entries.len(), MAX_CHT_ENTRIES);
    assert!(!document.entry(0).unwrap().is_selectable());
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::LimitReached)
    );
}

#[test]
fn deterministic_generated_malformed_input_is_panic_free_and_bounded() {
    // Dependency-free mutation loop covers every truncation point, malformed
    // key/value combinations, and arbitrary bytes using a fixed PRNG seed.
    let fixture = b"cheats = 3\ncheat0_desc = Lives\ncheat0_code = VALID\ncheat1_code = \"Broken\ncheat2_code = VALID\n";
    for end in 0..=fixture.len() {
        let _ = parse_cht_bytes(&fixture[..end]);
    }
    let mut seed = 0x12345678u32;
    for _ in 0..512 {
        let mut input = fixture.to_vec();
        for _ in 0..16 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let offset = seed as usize % input.len();
            input[offset] = (seed >> 24) as u8;
        }
        if let Ok(document) = parse_cht_bytes(&input) {
            assert!(document.entries.len() <= MAX_CHT_ENTRIES);
            assert!(document.warnings.len() <= MAX_CHT_DOCUMENT_WARNINGS);
            for entry in &document.entries {
                assert!(entry.warnings.len() <= MAX_CHT_ENTRY_WARNINGS);
                if !entry.is_selectable() {
                    assert!(ChtInstallEntry::from_entry(entry, true).is_none());
                }
            }
        }
    }
}

#[test]
fn padding_and_invalid_global_values_cannot_hide_duplicate_evidence() {
    let text = format!(
        "cheat0_code = A\n{}cheat0_code = B",
        " ".repeat(MAX_CHT_LINE_BYTES)
    );
    let document = parse(&text);
    assert!(!document.entries[0].is_selectable());
    assert!(
        document.entries[0]
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtEntryWarningKind::ConflictingDuplicate)
    );
    let document = parse("cheat0_code = A\ncustom = \"unfinished\ncustom = safe\n");
    assert_eq!(document.global_fields[0].1, "unfinished");
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::InvalidFieldValue)
    );
    assert!(
        document
            .warnings
            .iter()
            .any(|warning| warning.kind == ChtDocumentWarningKind::ConflictingDuplicate)
    );
    assert_eq!(document.selectable_count(), 1);
}

#[test]
fn complete_value_digests_distinguish_huge_duplicates_from_prefix_collisions() {
    let prefix = "界".repeat(30_000);
    for identical in [false, true] {
        let last = if identical { "X" } else { "Y" };
        let text = format!(
            "cheat0_desc=Lives\ncheat0_code=\"{prefix}X\"\ncheat0_code=\"{prefix}{last}\"\n"
        );
        let doc = parse_cht_text(&text).unwrap();
        assert_eq!(doc, parse_cht_text(&text).unwrap());
        let entry = &doc.entries[0];
        assert!(!entry.is_selectable());
        assert!(entry.code.as_ref().unwrap().len() <= MAX_CHT_FIELD_BYTES);
        assert!(
            entry
                .source_fields
                .iter()
                .all(|f| f.raw_source.len() <= MAX_CHT_LINE_BYTES
                    && f.value.len() <= MAX_CHT_FIELD_BYTES)
        );
        assert_eq!(
            entry
                .warnings
                .iter()
                .any(|w| w.kind == ChtEntryWarningKind::ConflictingDuplicate),
            !identical
        );
        assert_eq!(
            entry
                .warnings
                .iter()
                .any(|w| w.kind == ChtEntryWarningKind::DuplicateField),
            identical
        );
        let codes: Vec<_> = entry
            .source_fields
            .iter()
            .filter(|f| f.field == "code")
            .collect();
        assert_eq!(
            codes[0].full_value_sha256 == codes[1].full_value_sha256,
            identical
        );
    }
}

#[test]
fn huge_global_values_also_keep_complete_conflict_evidence() {
    let prefix = "A".repeat(64 * 1024);
    for identical in [false, true] {
        let last = if identical { "X" } else { "Y" };
        let doc = parse_cht_text(&format!(
            "author={prefix}X\nauthor={prefix}{last}\ncheat0_code=A\n"
        ))
        .unwrap();
        assert_eq!(
            doc.warnings
                .iter()
                .any(|w| w.kind == ChtDocumentWarningKind::ConflictingDuplicate),
            !identical
        );
    }
}
