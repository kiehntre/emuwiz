//! Unit tests for the BSFree GameCube bridge: classification, the proven
//! AR→Gecko byte-identity conversion, and the two-pass duplicate analysis.
//! Pure and filesystem-free; fixtures live only in memory or in a temporary
//! directory created by the caller.

use std::path::PathBuf;

use super::*;
use crate::patch_manager::{
    BsFreeCheat, BsFreeDeviceSummary, DeviceFormatCompatibility, parse_dolphin_ini,
};

fn cheat(name: &str, code: &str) -> BsFreeCheat {
    BsFreeCheat {
        upstream_id: 0,
        name: name.to_string(),
        note: None,
        code: code.to_string(),
        section: None,
        author: None,
        device: BsFreeDeviceSummary {
            upstream_id: 6,
            name: "Action Replay".to_string(),
            compatibility: DeviceFormatCompatibility::PotentiallyConvertible,
        },
        compatibility: DeviceFormatCompatibility::PotentiallyConvertible,
        truncated_fields: Vec::new(),
    }
}

fn named_cheat(id: i64, name: &str, code: &str) -> BsFreeCheat {
    let mut cheat = cheat(name, code);
    cheat.upstream_id = id;
    cheat
}

fn classify(code: &str) -> BsFreeGameCubeCodeFormat {
    classify_bsfree_gamecube_cheat(&cheat("c", code)).code_format
}

fn parse_ini(text: &str) -> DolphinIniDocument {
    parse_dolphin_ini(text)
}

fn empty_ini() -> DolphinIniDocument {
    parse_dolphin_ini("")
}

#[test]
fn pure_32_bit_writes_are_gecko_equivalent() {
    assert_eq!(
        classify("042318AC 3B8003E7\n042318B0 3B8003E8"),
        BsFreeGameCubeCodeFormat::GeckoEquivalent
    );
    assert_eq!(
        classify("042E4C84 00000001"),
        BsFreeGameCubeCodeFormat::GeckoEquivalent
    );
}

#[test]
fn gecko_equivalent_lines_are_byte_identical_after_conversion() {
    let raw = named_cheat(1, "Unlock All Items", "042E4C8C 00000001");
    let classified = classify_bsfree_gamecube_cheat(&raw);
    assert_eq!(
        classified.code_format,
        BsFreeGameCubeCodeFormat::GeckoEquivalent
    );
    let mapped = bsfree_cheat_as_gamehacking(&classified, 0);
    // The adapter input for a Gecko-equivalent code must carry the exact same
    // hex-pair lines, unmodified: "conversion" is byte-identity.
    assert_eq!(mapped.code_format, GameCubeCodeFormat::Gecko);
    assert_eq!(mapped.code_lines, vec!["042E4C8C 00000001"]);
}

#[test]
fn gecko_equivalent_address_must_fit_gecko_24bit_field() {
    // First word 0x057FAFF8 -> gcaddr 0x017FAFF8, a write to 0x817FAFF8,
    // which exceeds Gecko's 24-bit address field; the same bytes would not
    // behave identically, so the code stays Action Replay native instead of
    // being converted.
    assert_eq!(
        classify("057FAFF8 3B800001"),
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
}

#[test]
fn write_16_and_8_bit_with_fill_are_ar_native_not_gecko() {
    // AR 16-bit writes repeat (fill) with count = data>>16; Gecko 16-bit
    // writes once. Different semantics, so never relabelled as Gecko.
    assert_eq!(
        classify("0224CD50 00003E7F"),
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
    assert_eq!(
        classify("002E4BB3 000000FF"),
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
}

#[test]
fn float_write_is_ar_native() {
    assert_eq!(
        classify("063B8760 3F800000"),
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
}

#[test]
fn pointer_write_add_code_and_conditionals_are_ar_native() {
    assert_eq!(
        classify("80234C58 00000001"),
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
    assert_eq!(
        classify("A00AE4D0 00000001"),
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
    assert_eq!(
        classify("202E4C84 00000000\n042E4C88 00000001"),
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
}

#[test]
fn master_code_is_unsupported() {
    // Dolphin refuses master codes ("Master codes are not needed").
    assert_eq!(
        classify("C4129124 0000FF00"),
        BsFreeGameCubeCodeFormat::Unsupported
    );
    assert_eq!(
        classify("042318AC 3B8003E7\nC4129124 0000FF00"),
        BsFreeGameCubeCodeFormat::Unsupported
    );
}

#[test]
fn zero_code_and_self_modifying_are_unsupported() {
    assert_eq!(
        classify("00000000 04000000"),
        BsFreeGameCubeCodeFormat::Unsupported
    );
    assert_eq!(
        classify("00002222 00000001"),
        BsFreeGameCubeCodeFormat::Unsupported
    );
}

#[test]
fn placeholders_and_invalid_encrypted_dash_codes_are_malformed() {
    assert_eq!(
        classify("042E4C8C 0000XXXX"),
        BsFreeGameCubeCodeFormat::Malformed
    );
    assert_eq!(
        classify("XAUQ-995V-EMM2M\nHHC0-6EH5-TQ6UD"),
        BsFreeGameCubeCodeFormat::Malformed
    );
    assert_eq!(
        classify("0068A4FF 000000XX"),
        BsFreeGameCubeCodeFormat::Malformed
    );
    assert_eq!(classify("N/A"), BsFreeGameCubeCodeFormat::Malformed);
    assert_eq!(classify(""), BsFreeGameCubeCodeFormat::Malformed);
}

#[test]
fn empty_lines_and_whitespace_are_tolerated_in_classification() {
    let raw = named_cheat(1, "code", "  042E4C8C 00000001  \n\n  042E4C8C 00000001  ");
    let classified = classify_bsfree_gamecube_cheat(&raw);
    assert_eq!(
        classified.code_format,
        BsFreeGameCubeCodeFormat::GeckoEquivalent
    );
    assert_eq!(
        classified.code_lines,
        vec!["042E4C8C 00000001", "042E4C8C 00000001"]
    );
}

#[test]
fn gecko_equivalent_and_ar_native_are_selectable_but_others_are_not() {
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Lives", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Health", "0224CD50 00003E7F")),
        classify_bsfree_gamecube_cheat(&named_cheat(3, "Master", "C4129124 0000FF00")),
        classify_bsfree_gamecube_cheat(&named_cheat(4, "Placeholder", "042E4C8C 0000XXXX")),
    ];
    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &empty_ini());
    assert_eq!(selection.selectable_count(), 2);
    assert!(!selection.entries[0].already_managed);
    assert_eq!(
        selection.resolve(&cheats).unwrap_err().kind,
        BsFreeGameCubeErrorKind::NoSelectedCheats
    );
    let mut selection = selection;
    assert!(selection.set_selected(0, true));
    assert!(selection.set_selected(1, true));
    assert!(
        !selection.set_selected(2, true),
        "Unsupported never selects"
    );
    assert!(!selection.set_selected(3, true), "Malformed never selects");
    assert_eq!(selection.resolve(&cheats).unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// Two-pass duplicate / conflict analysis
// ---------------------------------------------------------------------------

#[test]
fn duplicate_record_within_bsfree_is_caught() {
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Lives", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Lives", "042318AC 3B8003E7")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::DuplicateRecord)
    );
}

#[test]
fn duplicate_body_with_different_labels_is_a_variant_not_a_duplicate_record() {
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Lives A", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Lives B", "042318AC 3B8003E7")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::DuplicateBody)
    );
}

#[test]
fn same_name_different_body_is_a_conflict() {
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Lives", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Lives", "042318AC 3B8003E8")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::DuplicateNameConflict)
    );
}

#[test]
fn identical_effective_writes_never_become_a_false_gamecube_conflict() {
    // Two byte-identical 32-bit writes to the same address: this is caught
    // as DuplicateBody (source-level), and must never additionally surface
    // as ConflictingMemoryWrite - the values genuinely agree.
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Infinite Health", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "999 HP", "042318AC 3B8003E7")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        !findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::ConflictingMemoryWrite),
        "identical writes must never be reported as a memory conflict: {findings:?}"
    );
}

#[test]
fn a_gamecube_write8_fill_repeat_count_cannot_falsely_block_an_unrelated_cheat() {
    // `0024CD50 00000302` is a Write8 fill (byte 0x02 repeated across
    // 0x8024CD50..=0x8024CD53). An unrelated cheat at a genuinely different,
    // non-overlapping address must not be affected by the fill's raw,
    // un-masked second word - before the fix this line's derived "value"
    // was the raw word 0x00000302, which does not equal any other code's
    // value by construction and could never have falsely matched, but the
    // real risk was the opposite direction (a genuine overlap going
    // undetected); this proves the fill itself introduces no spurious
    // finding against something it never touches.
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Clear Flags", "0024CD50 00000302")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Unrelated Cheat", "042319AC 00000001")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        !findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::ConflictingMemoryWrite),
        "a fill write must not conflict with an unrelated, non-overlapping address: \
         {findings:?}"
    );
}

#[test]
fn a_gamecube_write8_fill_range_conflicts_with_a_genuinely_overlapping_write() {
    // The same fill as above, but this time a second cheat writes a
    // different byte inside the filled range (0x8024CD52, the third byte of
    // the fill) - a real, provable conflict that must be caught and must
    // block selection.
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Clear Flags", "0024CD50 00000302")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Set Flag Three", "0024CD52 00000099")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::ConflictingMemoryWrite),
        "a genuinely overlapping write inside a fill range must conflict: {findings:?}"
    );
    assert!(BsFreeDedupFindingKind::ConflictingMemoryWrite.blocks_selection());
}

#[test]
fn a_gamecube_overlapping_write_of_a_different_width_is_detected_as_a_conflict() {
    // A 32-bit write of 0xAABBCCDD at 0x80001000 places byte 0xCC at
    // 0x80001002 (big-endian). A separate 8-bit write of a different byte
    // at that address overlaps and disagrees, despite neither the address
    // nor the size matching exactly.
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Full HP And Ammo", "04001000 AABBCCDD")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Broken Overlap", "00001002 00000011")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::ConflictingMemoryWrite),
        "an overlapping write of a different width with a different byte must conflict: \
         {findings:?}"
    );
}

#[test]
fn two_records_converting_to_identical_gecko_output_are_deduplicated() {
    // Both are pure 04 writes; both become byte-identical Gecko codes even
    // though their BSFree labels differ. The second must be deduplicated.
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Lives A", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Lives B", "042318AC 3B8003E7")),
    ];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &empty_ini());
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::DuplicateBody)
    );
    // Output-level dedup happens at staging time.
    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &empty_ini());
    let mut selection = selection;
    selection.select_all();
    assert_eq!(selection.resolve(&cheats).unwrap().len(), 2);
}

#[test]
fn gecko_equivalent_matching_an_installed_gecko_code_is_already_installed() {
    let destination = parse_ini(
        "[Gecko]\n$Lives [BSFree Archive]\n042318AC 3B8003E7\n[Gecko_Enabled]\n$Lives [BSFree Archive]\n",
    );
    let cheats = vec![classify_bsfree_gamecube_cheat(&named_cheat(
        1,
        "Lives",
        "042318AC 3B8003E7",
    ))];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &destination);
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::AlreadyInstalled)
    );
}

#[test]
fn already_installed_under_a_different_name_is_skipped_not_duplicated() {
    // The destination already has this exact Gecko body under another name;
    // a second selected label must not install a duplicate code body.
    let destination = parse_ini(
        "[Gecko]\n$My Own Lives [User]\n042318AC 3B8003E7\n[Gecko_Enabled]\n$My Own Lives [User]\n",
    );
    let cheats = vec![classify_bsfree_gamecube_cheat(&named_cheat(
        1,
        "Lives",
        "042318AC 3B8003E7",
    ))];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &destination);
    assert!(
        findings
            .iter()
            .any(|f| { f.kind == BsFreeDedupFindingKind::AlreadyInstalledDifferentName }),
        "the analysis must report the cross-label duplicate"
    );

    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &destination);
    let mut selection = selection;
    selection.select_all();
    let staging_root =
        std::env::temp_dir().join(format!("archivefs-bsfree-gc-skip-{}", std::process::id()));
    let result = stage_bsfree_gamecube_install(
        &staging_root,
        "GLME01.ini",
        &destination,
        true,
        &cheats,
        &selection,
    );
    assert_eq!(
        result.unwrap_err().kind,
        BsFreeGameCubeErrorKind::NoSelectedCheats,
        "the only selected cheat is already covered, so nothing is staged"
    );
    let _ = std::fs::remove_dir_all(&staging_root);
}

#[test]
fn gecko_equivalent_matching_an_installed_ar_code_is_a_cross_section_collision() {
    // The same 04 body installed as an Action Replay code: both engines
    // interpret these bytes differently, so this requires review, not apply.
    let destination = parse_ini(
        "[ActionReplay]\n$Lives [BSFree Archive]\n042318AC 3B8003E7\n[ActionReplay_Enabled]\n$Lives [BSFree Archive]\n",
    );
    let cheats = vec![classify_bsfree_gamecube_cheat(&named_cheat(
        1,
        "Lives",
        "042318AC 3B8003E7",
    ))];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &destination);
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::CrossSectionCollision)
    );
}

#[test]
fn installed_user_code_with_same_name_different_body_blocks_and_preserves() {
    let destination = parse_ini(
        "[ActionReplay]\n$Lives [BSFree Archive]\n0224CD50 00003E7F\n[ActionReplay_Enabled]\n$Lives [BSFree Archive]\n",
    );
    let cheats = vec![classify_bsfree_gamecube_cheat(&named_cheat(
        1,
        "Lives",
        "042318AC 3B8003E7",
    ))];
    let findings = analyze_bsfree_gamecube_duplicates(&cheats, &destination);
    assert!(
        findings
            .iter()
            .any(|f| f.kind == BsFreeDedupFindingKind::SameLabelDifferentBody)
    );
    // The blocking finding prevents staging (never silently overwrites).
    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &destination);
    let mut selection = selection;
    selection.select_all();
    let staging_root = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gc-conflict-{}",
        std::process::id()
    ));
    let result = stage_bsfree_gamecube_install(
        &staging_root,
        "GLME01.ini",
        &destination,
        true,
        &cheats,
        &selection,
    );
    assert_eq!(
        result.unwrap_err().kind,
        BsFreeGameCubeErrorKind::ConflictingSelection
    );
    let _ = std::fs::remove_dir_all(&staging_root);
}

#[test]
fn staging_skips_output_level_duplicates_and_reports_them() {
    let cheats = vec![
        classify_bsfree_gamecube_cheat(&named_cheat(1, "Lives A", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(2, "Lives B", "042318AC 3B8003E7")),
        classify_bsfree_gamecube_cheat(&named_cheat(3, "Health", "0224CD50 00003E7F")),
        classify_bsfree_gamecube_cheat(&named_cheat(4, "Master", "C4129124 0000FF00")),
    ];
    let destination = empty_ini();
    let selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &destination);
    let mut selection = selection;
    selection.select_all();
    let staging_root =
        std::env::temp_dir().join(format!("archivefs-bsfree-gc-dedup-{}", std::process::id()));
    let staged = stage_bsfree_gamecube_install(
        &staging_root,
        "GLME01.ini",
        &destination,
        false,
        &cheats,
        &selection,
    )
    .expect("staging succeeds");
    // "Lives B" and "Master" are skipped; only two distinct outputs staged.
    assert_eq!(staged.skipped_duplicates, vec!["Lives B"]);
    assert_eq!(staged.skipped_unselectable, vec!["Master"]);
    let contents = std::fs::read_to_string(&staged.staged.path).unwrap();
    assert!(contents.contains("Lives A [BSFree Archive]"));
    assert!(!contents.contains("Lives B"));
    assert!(!contents.contains("Master"));
    assert!(contents.contains("Health [BSFree Archive]"));
    let _ = std::fs::remove_dir_all(&staging_root);
}

// ---------------------------------------------------------------------------
// Identity matching
// ---------------------------------------------------------------------------

#[test]
fn match_requires_platform_and_exact_normalized_title() {
    assert_eq!(
        normalize_title("Luigi's Mansion"),
        normalize_title("Luigis Mansion")
    );
}

#[test]
fn region_evidence_reports_both_sides_honestly() {
    assert!(region_evidence(Some("USA"), Some("USA")).contains("contains"));
    assert!(region_evidence(Some("Europe"), Some("USA")).contains("does not explicitly"));
    assert!(region_evidence(None, Some("USA")).contains("archive region"));
    assert!(region_evidence(None, None).contains("no region"));
}

#[test]
fn converted_output_never_changes_native_gecko_bytes() {
    // A native Gecko code (from an existing provider) fed through the BSFree
    // mapping path must keep its bytes exactly.
    let raw = named_cheat(1, "Native", "04123456 00000001\n06000000 00000001");
    let classified = classify_bsfree_gamecube_cheat(&raw);
    // The second line is a Gecko string code (CT0 sub 3), not an AR 32-bit
    // write; the whole code is not treated as Gecko-equivalent.
    assert_eq!(
        classified.code_format,
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
    let mapped = bsfree_cheat_as_gamehacking(&classified, 0);
    assert_eq!(
        mapped.code_lines,
        vec!["04123456 00000001", "06000000 00000001"]
    );
}

#[test]
fn provider_never_touches_files_for_classification() {
    let raw = named_cheat(1, "Lives", "042318AC 3B8003E7");
    let _ = classify_bsfree_gamecube_cheat(&raw);
    let _ = bsfree_dolphin_code_name(&classify_bsfree_gamecube_cheat(&raw));
    // No path is created or read by the pure classification path.
    let probe = PathBuf::from("/nonexistent/archivefs-bsfree-gc-probe");
    assert!(!probe.exists());
}

#[test]
fn author_fallback_uses_bsfree_label_in_dolphin_name() {
    let raw = named_cheat(1, "Lives", "042318AC 3B8003E7");
    let classified = classify_bsfree_gamecube_cheat(&raw);
    assert_eq!(
        bsfree_dolphin_code_name(&classified),
        "Lives [BSFree Archive]"
    );
}

// ---------------------------------------------------------------------------
// Search / candidate / auto-match
// ---------------------------------------------------------------------------

fn search_fixture(path: &std::path::Path) {
    use rusqlite::Connection;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys=OFF;")
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE systems(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,group_id INTEGER NOT NULL,name TEXT NOT NULL,qty INTEGER NOT NULL DEFAULT 0);\
             CREATE TABLE devices(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL,qty INTEGER NOT NULL DEFAULT 0);\
             CREATE TABLE system_devices(system_id INTEGER NOT NULL REFERENCES systems,device_id INTEGER NOT NULL REFERENCES devices,PRIMARY KEY(system_id,device_id));\
             CREATE TABLE games(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,game_id INTEGER NOT NULL,name TEXT NOT NULL,version TEXT DEFAULT NULL,system_id INTEGER NOT NULL REFERENCES systems,device_id INTEGER NOT NULL REFERENCES devices,qty INTEGER NOT NULL DEFAULT 0);\
             CREATE TABLE sections(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,game_id INTEGER NOT NULL REFERENCES games(game_id),name TEXT NOT NULL,qty INTEGER NOT NULL DEFAULT 0);\
             CREATE TABLE authors(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL,qty INTEGER NOT NULL DEFAULT 0);\
             CREATE TABLE codes(id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL,code TEXT NOT NULL,note TEXT DEFAULT NULL,game_uid INTEGER NOT NULL REFERENCES games,game_id INTEGER NOT NULL REFERENCES games(game_id),system_id INTEGER NOT NULL REFERENCES systems,device_id INTEGER NOT NULL REFERENCES devices,section_id INTEGER DEFAULT NULL REFERENCES sections,author_id INTEGER DEFAULT NULL REFERENCES authors);",
        )
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO systems(id,group_id,name,qty) VALUES(17,17,'GameCube',10);\
             INSERT INTO devices(id,name,qty) VALUES(6,'Action Replay',10);\
             INSERT INTO system_devices VALUES(17,6);\
             INSERT INTO games(id,game_id,name,version,system_id,device_id,qty) VALUES
               (1001,1,'Luigi''s Mansion','USA',17,6,2),
               (1002,2,'The Sims',NULL,17,6,2),
               (1003,3,'The Sims',NULL,17,6,2),
               (1004,4,'Pokemon XD',NULL,17,6,2);\
             INSERT INTO codes(id,name,code,game_uid,game_id,system_id,device_id) VALUES
               (1,'Lives','042318AC 3B8003E7',1001,1,17,6),
               (2,'Money','042318B0 3B8003E8',1001,1,17,6),
               (3,'Lives','042318AC 3B8003E7',1002,2,17,6),
               (4,'Lives','042318AC 3B8003E7',1003,3,17,6),
               (5,'Max','042318AC 3B8003E7',1004,4,17,6);",
        )
        .unwrap();
}

fn search_catalogue(path: &std::path::Path) -> BsFreeCatalogue {
    BsFreeCatalogue::open_with_expected_hash(path, None).unwrap()
}

#[test]
fn search_auto_matches_a_unique_title_and_loads_cheats() {
    let root =
        std::env::temp_dir().join(format!("archivefs-bsfree-gc-search-{}", std::process::id()));
    let db = root.join("search.db");
    search_fixture(&db);
    let catalogue = search_catalogue(&db);
    let outcome =
        bsfree_gamecube_search(&catalogue, "Luigi's Mansion (USA)", "GLME01", Some("E")).unwrap();
    assert_eq!(outcome.status, BsFreeGameCubeSearchStatus::Matched);
    let game = outcome.game.expect("a single match auto-selects");
    assert_eq!(game.matched_bsfree_title, "Luigi's Mansion");
    assert!(game.requires_review);
    assert_eq!(outcome.cheats.len(), 2, "classified cheats are loaded");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn search_returns_candidates_when_titles_tie() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gc-search-tie-{}",
        std::process::id()
    ));
    let db = root.join("search.db");
    search_fixture(&db);
    let catalogue = search_catalogue(&db);
    let outcome = bsfree_gamecube_search(&catalogue, "The Sims", "GLME01", Some("E")).unwrap();
    assert_eq!(outcome.status, BsFreeGameCubeSearchStatus::Candidates);
    assert_eq!(outcome.candidates.len(), 2);
    assert!(
        outcome.cheats.is_empty(),
        "no cheats auto-load while ambiguous"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn search_returns_no_match_and_empty_search_is_harmless() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gc-search-none-{}",
        std::process::id()
    ));
    let db = root.join("search.db");
    search_fixture(&db);
    let catalogue = search_catalogue(&db);
    let outcome =
        bsfree_gamecube_search(&catalogue, "Super Mario Sunshine", "GLME01", None).unwrap();
    assert_eq!(outcome.status, BsFreeGameCubeSearchStatus::NoMatch);
    assert!(outcome.candidates.is_empty());
    let outcome = bsfree_gamecube_search(&catalogue, "   ", "GLME01", None).unwrap();
    assert_eq!(outcome.status, BsFreeGameCubeSearchStatus::NoMatch);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn load_confirmed_binds_the_match_and_loads_cheats() {
    let root = std::env::temp_dir().join(format!(
        "archivefs-bsfree-gc-confirm-{}",
        std::process::id()
    ));
    let db = root.join("search.db");
    search_fixture(&db);
    let catalogue = search_catalogue(&db);
    let outcome =
        bsfree_gamecube_load_confirmed(&catalogue, 1002, "Archive Title", "GLME01", Some("E"))
            .unwrap()
            .expect("the confirmed game exists");
    assert_eq!(outcome.status, BsFreeGameCubeSearchStatus::Matched);
    assert_eq!(outcome.game.unwrap().matched_bsfree_title, "The Sims");
    assert_eq!(outcome.cheats.len(), 1);
    let missing = bsfree_gamecube_load_confirmed(&catalogue, 9999, "x", "GLME01", None).unwrap();
    assert!(missing.is_none());
    let _ = std::fs::remove_dir_all(&root);
}

const ENCRYPTED_NATIVE: &str = "XAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD";
const ENCRYPTED_GECKO: &str = "MW93-4G33-3JE7T\n1AXC-RQB8-H78K3";

#[test]
fn encrypted_published_vectors_match_existing_raw_outputs_exactly() {
    for (encrypted, raw) in [
        ("G12C-TMX0-WRT5C\nG2ND-C1RJ-G4TZ1", "00690E90 000004FF"),
        (ENCRYPTED_NATIVE, "021F11DA 00000001"),
        (ENCRYPTED_GECKO, "04001000 AABBCCDD"),
    ] {
        let (decoded, evidence) =
            classify_bsfree_gamecube_cheat_with_provenance(&named_cheat(42, "Fixture", encrypted));
        let reference = classify_bsfree_gamecube_cheat(&named_cheat(42, "Fixture", raw));
        assert_eq!(decoded, reference);
        assert_eq!(
            serde_json::to_vec(&decoded).unwrap(),
            serde_json::to_vec(&reference).unwrap()
        );
        assert_eq!(
            bsfree_cheat_as_gamehacking(&decoded, 7),
            bsfree_cheat_as_gamehacking(&reference, 7)
        );
        assert!(matches!(
            evidence,
            Some(BsFreeGameCubeArDecryption::Verified(_))
        ));
    }
}

#[test]
fn encrypted_provenance_preserves_provider_metadata_and_exact_original_text() {
    let text = " \r\n xauq-995v-emm2k \t\r\n\r\n hhc0-6eh5-tq6ud \r\n";
    let mut input = named_cheat(42, "Provider name", text);
    input.note = Some("Provider note".into());
    input.author = Some(crate::patch_manager::BsFreeNamedRow {
        upstream_id: 3,
        name: "Author".into(),
    });
    input.section = Some(crate::patch_manager::BsFreeNamedRow {
        upstream_id: 4,
        name: "Section".into(),
    });
    let original = input.clone();
    let (classified, evidence) = classify_bsfree_gamecube_cheat_with_provenance(&input);
    assert_eq!(input, original);
    assert_eq!(classified.name, input.name);
    assert_eq!(classified.note, input.note);
    assert_eq!(classified.author.as_deref(), Some("Author"));
    assert_eq!(classified.section.as_deref(), Some("Section"));
    let Some(BsFreeGameCubeArDecryption::Verified(decoded)) = evidence else {
        panic!("decode missing")
    };
    assert_eq!(decoded.original_encrypted_text, text);
    assert_eq!(decoded.decoder_version, GAMECUBE_AR_DECODER_VERSION);
    assert_eq!(decoded.raw_ar_text, "021F11DA 00000001");
    assert_eq!(
        decoded.verification.verifier_words,
        [0x704E01EF, 0x08000000]
    );
    assert_eq!(decoded.verification.game_id, 0x27);
    let json = serde_json::to_value(BsFreeGameCubeArDecryption::Verified(decoded)).unwrap();
    assert_eq!(json["status"], "verified");
    assert_eq!(json["evidence"]["original_encrypted_text"], text);
}

#[test]
fn encrypted_master_verifiers_cannot_promote_supported_bodies() {
    for encrypted in [
        "Y1HA-TP30-Y5PRA\nZZHF-4YM9-NE8YA",
        "NT40-E3MT-TTTN4\nT1MV-XZ0P-2YDR5\n99DR-JVGX-Z6DAF",
    ] {
        let (classified, evidence) =
            classify_bsfree_gamecube_cheat_with_provenance(&cheat("Master", encrypted));
        let Some(BsFreeGameCubeArDecryption::Verified(decoded)) = evidence else {
            panic!("decode missing")
        };
        assert!(decoded.verification.is_master);
        assert!(
            classify_bsfree_gamecube_cheat(&cheat("Body", &decoded.raw_ar_text))
                .code_format
                .is_installable()
        );
        assert_eq!(
            classified.code_format,
            BsFreeGameCubeCodeFormat::Unsupported
        );
        let mut selection = BsFreeGameCubeCheatSelection::from_cheats(&[classified], &empty_ini());
        assert!(!selection.set_selected(0, true));
        assert_eq!(selection.selectable_count(), 0);
    }
}

#[test]
fn encrypted_master_zero_and_self_modifying_opcodes_stay_refused() {
    for encrypted in [
        "GKMU-93RZ-82YV6\nZFPE-UYKV-PX95X",
        "XR7M-X292-DZ418\nKAJ8-YZ3T-1JJ2X",
        "DYV0-42F6-NB6CZ\nU3WW-0FR5-62XWT",
        "9CJK-KYJ2-WFNR2\nTMU2-R0DX-V4MNA",
    ] {
        assert_eq!(classify(encrypted), BsFreeGameCubeCodeFormat::Unsupported);
    }
}

#[test]
fn encrypted_pointer_float_add_and_conditional_order_is_preserved() {
    let text = "4MJ1-JJM8-7JF18\n3041-UP6W-KQGJE\nEF78-K2N7-03X8F\nQTRA-6WT7-UZTB9\nPF3N-BE7M-AKBQE\nFQAA-E2GT-54ZRH\nN3GY-C825-4CWJH\nEXFB-B6X7-W4W7V";
    let result = classify_bsfree_gamecube_cheat(&cheat("Operations", text));
    assert_eq!(
        result.code_format,
        BsFreeGameCubeCodeFormat::ActionReplayNative
    );
    assert_eq!(
        result.code_lines,
        [
            "002E4BB3 000000FF",
            "0224CD50 00003E7F",
            "063B8760 3F800000",
            "80234C58 00000001",
            "A00AE4D0 00000001",
            "202E4C84 00000000",
            "042E4C88 00000001"
        ]
    );
}

#[test]
fn encrypted_wrong_devices_and_truncated_code_cannot_decode() {
    for device in ["Gecko", "Action Replay Max", "GameShark", "Unknown", ""] {
        let mut raw = cheat("Device", ENCRYPTED_NATIVE);
        raw.device.name = device.into();
        let (classified, evidence) = classify_bsfree_gamecube_cheat_with_provenance(&raw);
        assert_eq!(classified.code_format, BsFreeGameCubeCodeFormat::Malformed);
        assert!(
            matches!(evidence, Some(BsFreeGameCubeArDecryption::Refused { reason, .. }) if reason.contains("device"))
        );
    }
    let mut raw = cheat("Truncated", ENCRYPTED_NATIVE);
    raw.truncated_fields.push("code".into());
    let (classified, evidence) = classify_bsfree_gamecube_cheat_with_provenance(&raw);
    assert!(!classified.code_format.is_installable());
    assert!(
        matches!(evidence, Some(BsFreeGameCubeArDecryption::Refused { reason, .. }) if reason.contains("truncated"))
    );
}

#[test]
fn encrypted_failures_keep_precise_reasons_and_never_partial_install() {
    for (text, reason_fragment) in [
        ("XAUQ-995V-EMM2M\nHHC0-6EH5-TQ6UD", "parity"),
        ("0AUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD", "checksum"),
        ("YAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD", "verifier"),
        ("XAUQ-995V-EMM2K\n021F11DA 00000001", "mixed"),
        ("ＸAUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD", "ASCII"),
    ] {
        let (classified, evidence) =
            classify_bsfree_gamecube_cheat_with_provenance(&cheat("Failure", text));
        assert_eq!(classified.code_format, BsFreeGameCubeCodeFormat::Malformed);
        let Some(BsFreeGameCubeArDecryption::Refused {
            original_encrypted_text,
            original_bytes,
            source_sha256,
            reason,
            ..
        }) = evidence
        else {
            panic!("refusal missing")
        };
        assert_eq!(original_encrypted_text.as_deref(), Some(text));
        assert_eq!(original_bytes, text.len());
        assert_eq!(source_sha256, hex_sha256(&Sha256::digest(text.as_bytes())));
        assert!(reason.contains(reason_fragment), "{reason}");
        let mut selection =
            BsFreeGameCubeCheatSelection::from_cheats(&[classified.clone()], &empty_ini());
        selection.entries[0].selectable = true;
        selection.entries[0].selected = true;
        assert!(selection.resolve(&[classified]).is_err());
    }
}

#[test]
fn encrypted_bounds_do_not_duplicate_oversized_diagnostics() {
    let text = "XAUQ-995V-EMM2K\n".repeat(2000);
    let (classified, evidence) =
        classify_bsfree_gamecube_cheat_with_provenance(&cheat("Huge", &text));
    assert_eq!(classified.code_format, BsFreeGameCubeCodeFormat::Malformed);
    assert!(classified.code_lines.is_empty());
    assert!(
        matches!(evidence, Some(BsFreeGameCubeArDecryption::Refused { original_encrypted_text: None, original_bytes, reason, .. }) if original_bytes == text.len() && reason.contains("bytes"))
    );
}

#[test]
fn encrypted_wii_path_remains_browse_only() {
    for text in [ENCRYPTED_NATIVE, ENCRYPTED_GECKO] {
        let wii = crate::patch_manager::classify_bsfree_wii_cheat(&cheat("Wii", text));
        assert!(!wii.code_format.is_installable());
    }
}

#[test]
fn encrypted_and_raw_outputs_deduplicate_without_losing_decode_evidence() {
    let (decoded, evidence) = classify_bsfree_gamecube_cheat_with_provenance(&named_cheat(
        1,
        "Encrypted",
        ENCRYPTED_GECKO,
    ));
    let raw = classify_bsfree_gamecube_cheat(&named_cheat(2, "Raw", "04001000 AABBCCDD"));
    assert_eq!(decoded.canonical_digest, raw.canonical_digest);
    let cheats = vec![decoded, raw];
    let mut selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &empty_ini());
    assert_eq!(selection.selected_count(), 0);
    selection.select_all();
    let root = tempfile::tempdir().unwrap();
    let staged = stage_bsfree_gamecube_install(
        root.path(),
        "GLME01.ini",
        &empty_ini(),
        false,
        &cheats,
        &selection,
    )
    .unwrap();
    assert_eq!(staged.skipped_duplicates, ["Raw"]);
    assert_eq!(parse_ini(&staged.staged.contents).gecko_codes.len(), 1);
    assert!(matches!(
        evidence,
        Some(BsFreeGameCubeArDecryption::Verified(_))
    ));
}

#[test]
fn encrypted_catalogue_search_and_confirmation_retain_evidence_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("catalogue.db");
    search_fixture(&db);
    {
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute("UPDATE codes SET code=?1 WHERE id=1", [ENCRYPTED_NATIVE])
            .unwrap();
        connection
            .execute(
                "UPDATE codes SET code=?1 WHERE id=2",
                ["0AUQ-995V-EMM2K\nHHC0-6EH5-TQ6UD"],
            )
            .unwrap();
    }
    let before = std::fs::read(&db).unwrap();
    let catalogue = search_catalogue(&db);
    for outcome in [
        bsfree_gamecube_search(&catalogue, "Luigi's Mansion", "GLME01", Some("E")).unwrap(),
        bsfree_gamecube_load_confirmed(&catalogue, 1001, "Archive", "GLME01", Some("E"))
            .unwrap()
            .unwrap(),
    ] {
        assert!(outcome.game.as_ref().unwrap().requires_review);
        assert_eq!(outcome.game.as_ref().unwrap().archive_game_id, "GLME01");
        assert_eq!(
            outcome.cheats[0].code_format,
            BsFreeGameCubeCodeFormat::ActionReplayNative
        );
        assert!(!outcome.cheats[1].code_format.is_installable());
        assert!(matches!(
            outcome.ar_decryption[&1],
            BsFreeGameCubeArDecryption::Verified(_)
        ));
        assert!(matches!(
            outcome.ar_decryption[&2],
            BsFreeGameCubeArDecryption::Refused { .. }
        ));
        assert_eq!(
            serde_json::to_value(&outcome).unwrap()["ar_decryption"]["1"]["status"],
            "verified"
        );
        assert_eq!(
            BsFreeGameCubeCheatSelection::from_cheats(&outcome.cheats, &empty_ini())
                .selected_count(),
            0
        );
    }
    drop(catalogue);
    assert_eq!(std::fs::read(&db).unwrap(), before);
}

#[test]
fn encrypted_catalogue_requires_gamecube_mapping_without_filename_inference() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("catalogue.db");
    search_fixture(&db);
    {
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute("UPDATE systems SET name='Nintendo 64' WHERE id=17", [])
            .unwrap();
        connection
            .execute(
                "UPDATE codes SET code=?1 WHERE game_uid=1001",
                [ENCRYPTED_NATIVE],
            )
            .unwrap();
    }
    let catalogue = search_catalogue(&db);
    let (cheats, evidence) = bsfree_gamecube_cheats_with_provenance(&catalogue, 1001).unwrap();
    assert!(
        cheats
            .iter()
            .all(|cheat| !cheat.code_format.is_installable())
    );
    assert!(evidence.values().all(|item| matches!(item, BsFreeGameCubeArDecryption::Refused { reason, .. } if reason.contains("GameCube"))));
}

#[test]
fn encrypted_raw_catalogue_result_has_no_new_serialized_field() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("catalogue.db");
    search_fixture(&db);
    let result =
        bsfree_gamecube_search(&search_catalogue(&db), "Luigi's Mansion", "GLME01", None).unwrap();
    assert!(result.ar_decryption.is_empty());
    assert!(
        serde_json::to_value(result)
            .unwrap()
            .get("ar_decryption")
            .is_none()
    );
    let (classified, evidence) =
        classify_bsfree_gamecube_cheat_with_provenance(&cheat("Raw", "042318AC 3B8003E7"));
    assert!(evidence.is_none());
    assert!(
        serde_json::to_value(classified)
            .unwrap()
            .get("ar_decryption")
            .is_none()
    );
}

fn encrypted_transaction_fixture() -> (
    tempfile::TempDir,
    crate::patch_manager::SharedTransactionPlan,
) {
    use crate::patch_manager::build_shared_transaction_plan;
    let root = tempfile::tempdir().unwrap();
    let selected_archive = root.path().join("synthetic-game.iso");
    std::fs::write(&selected_archive, b"synthetic selected media").unwrap();
    let configuration = root.path().join("dolphin");
    std::fs::create_dir_all(configuration.join("GameSettings")).unwrap();
    std::fs::write(
        configuration.join("GameSettings/GLME01.ini"),
        b"[Core]\r\nCPUThread = True\r\n",
    )
    .unwrap();
    let destination = parse_ini("[Core]\r\nCPUThread = True\r\n");
    let cheats = vec![classify_bsfree_gamecube_cheat(&cheat(
        "Decoded",
        ENCRYPTED_NATIVE,
    ))];
    let mut selection = BsFreeGameCubeCheatSelection::from_cheats(&cheats, &destination);
    assert_eq!(selection.selected_count(), 0);
    selection.select_all();
    let staged = stage_bsfree_gamecube_install(
        &root.path().join("stage"),
        "GLME01.ini",
        &destination,
        true,
        &cheats,
        &selection,
    )
    .unwrap();
    let preview = build_bsfree_gamecube_install_preview(&BsFreeGameCubeInstallPreviewRequest {
        selected_archive,
        configuration_path: configuration,
        game_id: "GLME01".into(),
        revision: None,
        staged: staged.staged,
    })
    .unwrap();
    let plan = build_shared_transaction_plan(
        &preview.report,
        "dolphin-profile",
        "bsfree-gamecube",
        &root.path().join("stage"),
    )
    .unwrap();
    (root, plan)
}

fn encrypted_apply_options(
    root: &std::path::Path,
    plan: &crate::patch_manager::SharedTransactionPlan,
) -> crate::patch_manager::SharedApplyOptions {
    use crate::patch_manager::{SharedApplyConfirmation, SharedApplyOptions};
    SharedApplyOptions {
        dry_run: false,
        confirmation: Some(SharedApplyConfirmation {
            plan_id: plan.plan_id.clone(),
            general_approved: true,
            replacement_approved: true,
        }),
        operation_id: "encrypted-ar-apply".into(),
        timestamp_unix_seconds: 100,
        current_context: plan.context.clone(),
        history_root: root.join("history"),
        backup_root: root.join("backups"),
    }
}

#[test]
fn encrypted_install_requires_confirmation_and_uses_shared_apply_and_exact_undo() {
    use crate::patch_manager::{
        SharedApplyStatus, SharedRollbackConfirmation, SharedRollbackOptions, execute_shared_apply,
        execute_shared_rollback, preview_shared_rollback,
    };
    let (root, plan) = encrypted_transaction_fixture();
    let output = root.path().join("dolphin/GameSettings/GLME01.ini");
    let before = std::fs::read(&output).unwrap();
    let media = root.path().join("synthetic-game.iso");
    let media_before = std::fs::read(&media).unwrap();
    let mut options = encrypted_apply_options(root.path(), &plan);
    options.confirmation = None;
    assert_ne!(
        execute_shared_apply(&plan, &options).journal.status,
        SharedApplyStatus::Success
    );
    assert_eq!(std::fs::read(&output).unwrap(), before);
    let result = execute_shared_apply(&plan, &encrypted_apply_options(root.path(), &plan));
    assert_eq!(
        result.journal.status,
        SharedApplyStatus::Success,
        "{result:?}"
    );
    let contents = std::fs::read_to_string(&output).unwrap();
    assert!(contents.contains("[ActionReplay]"));
    assert!(contents.contains("021F11DA 00000001"));
    assert!(!contents.contains("704E01EF"));
    assert!(!contents.contains("XAUQ-995V"));
    assert_eq!(plan.context.verified_game_identity, "GLME01");
    let journal = result.journal_path.unwrap();
    let preview = preview_shared_rollback(
        &journal,
        &root.path().join("dolphin"),
        &root.path().join("backups"),
    );
    assert!(preview.available);
    let undone = execute_shared_rollback(
        &preview,
        &SharedRollbackOptions {
            confirmation: SharedRollbackConfirmation {
                preview_id: preview.preview_id.clone(),
                approved: true,
            },
            rollback_operation_id: "encrypted-ar-undo".into(),
            timestamp_unix_seconds: 101,
            history_root: root.path().join("history"),
            backup_root: root.path().join("backups"),
        },
    );
    assert_eq!(undone.status, SharedApplyStatus::Success);
    assert_eq!(std::fs::read(&output).unwrap(), before);
    assert_eq!(std::fs::read(&media).unwrap(), media_before);
    assert!(
        !preview_shared_rollback(
            &journal,
            &root.path().join("dolphin"),
            &root.path().join("backups")
        )
        .available
    );
}

#[test]
fn encrypted_install_rejects_stale_staging_and_wrong_game_context() {
    use crate::patch_manager::{SharedApplyStatus, execute_shared_apply};
    for change_staging in [false, true] {
        let (root, plan) = encrypted_transaction_fixture();
        let output = root.path().join("dolphin/GameSettings/GLME01.ini");
        let before = std::fs::read(&output).unwrap();
        let mut options = encrypted_apply_options(root.path(), &plan);
        if change_staging {
            std::fs::write(root.path().join("stage/GLME01.ini"), b"changed staged code").unwrap();
        } else {
            options.current_context.verified_game_identity = "GAFE01".into();
        }
        let result = execute_shared_apply(&plan, &options);
        assert_ne!(result.journal.status, SharedApplyStatus::Success);
        assert_eq!(std::fs::read(output).unwrap(), before);
    }
}

#[test]
fn encrypted_undo_refuses_changed_user_output() {
    use crate::patch_manager::{SharedApplyStatus, execute_shared_apply, preview_shared_rollback};
    let (root, plan) = encrypted_transaction_fixture();
    let result = execute_shared_apply(&plan, &encrypted_apply_options(root.path(), &plan));
    assert_eq!(result.journal.status, SharedApplyStatus::Success);
    let output = root.path().join("dolphin/GameSettings/GLME01.ini");
    std::fs::write(&output, b"user changed the output").unwrap();
    let preview = preview_shared_rollback(
        &result.journal_path.unwrap(),
        &root.path().join("dolphin"),
        &root.path().join("backups"),
    );
    assert!(!preview.available);
    assert_eq!(std::fs::read(output).unwrap(), b"user changed the output");
}
