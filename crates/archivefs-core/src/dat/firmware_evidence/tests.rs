//! Tests drive the real DAT parser (`parse_dat_file`) over real temp files -
//! never a hand-built `ParsedDat` - so these prove the same code path a
//! genuine user-supplied Redump PS2 BIOS DAT would go through. No network,
//! no embedded Redump hash records: every fixture hash below is a
//! synthetic, self-consistent placeholder invented for this test only.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::dat::limits::DatLimits;
use crate::dat::parsers::parse_dat_file;

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn write_dat(xml: &str) -> (std::path::PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let path = dir.path().join(format!("firmware-evidence-{sequence}.dat"));
    fs::write(&path, xml).unwrap();
    (path, dir)
}

fn parsed(xml: &str) -> ParsedDat {
    let (path, _dir) = write_dat(xml);
    parse_dat_file(&path, DatLimits::default()).unwrap().dat
}

const PS2_BIOS_HEADER: &str = r#"
    <header>
        <name>Sony - PlayStation 2 - BIOS Images</name>
        <description>Sony - PlayStation 2 - BIOS Images</description>
        <version>20240101</version>
        <author>Redump.org</author>
    </header>"#;

#[test]
fn genuine_redump_ps2_bios_dat_yields_records() {
    let xml = format!(
        r#"<?xml version="1.0"?>
<datafile>{PS2_BIOS_HEADER}
    <game name="Sony PlayStation 2 BIOS v02.20(10/02/2005) Console">
        <description>Sony PlayStation 2 BIOS v02.20(10/02/2005) Console</description>
        <rom name="scph-70012.bin" size="4194304" crc="aabbccdd" md5="00112233445566778899aabbccddeeff" sha1="0011223344556677889900112233445566778899"/>
    </game>
</datafile>"#
    );
    let dat = parsed(&xml);
    let records = ps2_bios_evidence_from_dat(&dat).unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.system, FirmwareSystem::PlayStation2);
    assert_eq!(record.provider, DatEcosystem::Redump);
    assert_eq!(
        record.name,
        "Sony PlayStation 2 BIOS v02.20(10/02/2005) Console"
    );
    assert_eq!(
        record.description.as_deref(),
        Some("Sony PlayStation 2 BIOS v02.20(10/02/2005) Console")
    );
    assert_eq!(record.size_bytes, 4_194_304);
    assert_eq!(record.crc32, "aabbccdd");
    assert_eq!(record.dat_version.as_deref(), Some("20240101"));
}

#[test]
fn non_redump_dat_is_rejected() {
    let xml = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Sony - PlayStation 2 - BIOS Images</name>
    </header>
    <game name="fixture">
        <rom name="scph-70012.bin" size="4194304" crc="aabbccdd" md5="00112233445566778899aabbccddeeff" sha1="0011223344556677889900112233445566778899"/>
    </game>
</datafile>"#;
    let dat = parsed(xml);
    assert_eq!(dat.source.ecosystem, DatEcosystem::GenericLogiqx);
    let error = ps2_bios_evidence_from_dat(&dat).unwrap_err();
    assert_eq!(error, FirmwareEvidenceError::NotRedump);
}

#[test]
fn redump_dat_for_a_different_dataset_is_rejected() {
    let xml = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Redump - Sony PlayStation 2</name>
    </header>
    <game name="Some Game (USA)">
        <rom name="game.iso" size="4700000000" crc="aabbccdd" md5="00112233445566778899aabbccddeeff" sha1="0011223344556677889900112233445566778899"/>
    </game>
</datafile>"#;
    let dat = parsed(xml);
    assert_eq!(dat.source.ecosystem, DatEcosystem::Redump);
    let error = ps2_bios_evidence_from_dat(&dat).unwrap_err();
    assert_eq!(error, FirmwareEvidenceError::NotBiosDataset);
}

#[test]
fn redump_ps2_bios_dat_with_no_complete_records_is_rejected() {
    let xml = format!(
        r#"<?xml version="1.0"?>
<datafile>{PS2_BIOS_HEADER}
    <game name="Sony PlayStation 2 BIOS incomplete">
        <rom name="scph-70012.bin" size="4194304" crc="aabbccdd"/>
    </game>
</datafile>"#
    );
    let dat = parsed(&xml);
    let error = ps2_bios_evidence_from_dat(&dat).unwrap_err();
    assert_eq!(error, FirmwareEvidenceError::NoUsableEntries);
}

#[test]
fn incomplete_records_are_dropped_but_complete_siblings_still_produce_evidence() {
    let xml = format!(
        r#"<?xml version="1.0"?>
<datafile>{PS2_BIOS_HEADER}
    <game name="incomplete">
        <rom name="missing-sha1.bin" size="4194304" crc="aabbccdd" md5="00112233445566778899aabbccddeeff"/>
    </game>
    <game name="complete">
        <rom name="scph-70012.bin" size="4194304" crc="aabbccdd" md5="00112233445566778899aabbccddeeff" sha1="0011223344556677889900112233445566778899"/>
    </game>
</datafile>"#
    );
    let dat = parsed(&xml);
    let records = ps2_bios_evidence_from_dat(&dat).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].name, "complete");
}

// ---------------------------------------------------------------------------
// PS1 / Xbox generalization (redump_bios_evidence_from_dat)
// ---------------------------------------------------------------------------

const PS1_BIOS_HEADER: &str = r#"
    <header>
        <name>Sony - PlayStation - BIOS Images</name>
        <description>Sony - PlayStation - BIOS Images</description>
        <version>20240101</version>
        <author>Redump.org</author>
    </header>"#;

const XBOX_BIOS_HEADER: &str = r#"
    <header>
        <name>Microsoft - Xbox - BIOS Images</name>
        <description>Microsoft - Xbox - BIOS Images</description>
        <version>20240101</version>
        <author>Redump.org</author>
    </header>"#;

fn one_bios_record_xml(header: &str, game_name: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<datafile>{header}
    <game name="{game_name}">
        <description>{game_name}</description>
        <rom name="bios.bin" size="524288" crc="aabbccdd" md5="00112233445566778899aabbccddeeff" sha1="0011223344556677889900112233445566778899"/>
    </game>
</datafile>"#
    )
}

#[test]
fn genuine_redump_ps1_bios_dat_yields_playstation_evidence() {
    let xml = one_bios_record_xml(PS1_BIOS_HEADER, "PS1 BIOS (SCPH-1001)");
    let dat = parsed(&xml);
    let records = redump_bios_evidence_from_dat(&dat, FirmwareSystem::PlayStation).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].system, FirmwareSystem::PlayStation);
    assert_eq!(records[0].provider, DatEcosystem::Redump);
}

#[test]
fn genuine_redump_xbox_bios_dat_yields_xbox_evidence() {
    let xml = one_bios_record_xml(XBOX_BIOS_HEADER, "Xbox BIOS (v1.0)");
    let dat = parsed(&xml);
    let records = redump_bios_evidence_from_dat(&dat, FirmwareSystem::Xbox).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].system, FirmwareSystem::Xbox);
}

#[test]
fn ps1_descriptor_rejects_a_ps2_bios_dat() {
    let xml = one_bios_record_xml(PS2_BIOS_HEADER, "PS2 BIOS");
    let dat = parsed(&xml);
    let error = redump_bios_evidence_from_dat(&dat, FirmwareSystem::PlayStation).unwrap_err();
    assert_eq!(error, FirmwareEvidenceError::NotBiosDataset);
}

#[test]
fn ps2_descriptor_rejects_a_xbox_bios_dat() {
    let xml = one_bios_record_xml(XBOX_BIOS_HEADER, "Xbox BIOS");
    let dat = parsed(&xml);
    let error = redump_bios_evidence_from_dat(&dat, FirmwareSystem::PlayStation2).unwrap_err();
    assert_eq!(error, FirmwareEvidenceError::NotBiosDataset);
}

#[test]
fn xbox_descriptor_rejects_a_ps1_bios_dat() {
    let xml = one_bios_record_xml(PS1_BIOS_HEADER, "PS1 BIOS");
    let dat = parsed(&xml);
    let error = redump_bios_evidence_from_dat(&dat, FirmwareSystem::Xbox).unwrap_err();
    assert_eq!(error, FirmwareEvidenceError::NotBiosDataset);
}

#[test]
fn arbitrary_redump_games_dat_is_rejected_for_every_system() {
    let xml = r#"<?xml version="1.0"?>
<datafile>
    <header>
        <name>Sony - PlayStation 2</name>
        <description>Sony - PlayStation 2 Datfile (full game discs)</description>
        <author>Redump.org</author>
    </header>
    <game name="Some Game">
        <rom name="game.bin" size="1" crc="00000000" md5="00000000000000000000000000000000" sha1="0000000000000000000000000000000000000000"/>
    </game>
</datafile>"#;
    let dat = parsed(xml);
    assert_eq!(dat.source.ecosystem, DatEcosystem::Redump);
    for system in [
        FirmwareSystem::PlayStation,
        FirmwareSystem::PlayStation2,
        FirmwareSystem::Xbox,
    ] {
        assert_eq!(
            redump_bios_evidence_from_dat(&dat, system).unwrap_err(),
            FirmwareEvidenceError::NotBiosDataset
        );
    }
}

#[test]
fn empty_dataset_is_rejected() {
    let xml = format!(
        r#"<?xml version="1.0"?>
<datafile>{PS2_BIOS_HEADER}
</datafile>"#
    );
    let dat = parsed(&xml);
    let error = redump_bios_evidence_from_dat(&dat, FirmwareSystem::PlayStation2).unwrap_err();
    assert_eq!(error, FirmwareEvidenceError::NoUsableEntries);
}

/// Compile-time proof, not just a runtime assertion: [`FirmwareSystem`] has
/// exactly these three variants - no MCPX/EEPROM (or any other
/// sub-component) variant exists for this match to silently ignore via a
/// wildcard arm. A future variant addition would fail to compile here until
/// this match (and this comment) is updated, which is the point.
#[test]
fn firmware_system_has_no_mcpx_or_eeprom_variant() {
    fn describe(system: FirmwareSystem) -> &'static str {
        match system {
            FirmwareSystem::PlayStation => "playstation",
            FirmwareSystem::PlayStation2 => "playstation2",
            FirmwareSystem::Xbox => "xbox-bios-flash-only",
        }
    }
    assert_eq!(describe(FirmwareSystem::Xbox), "xbox-bios-flash-only");
}

#[test]
fn xbox_redump_dataset_label_names_bios_images_only() {
    // The label used for dataset-identity matching/error text names exactly
    // what Redump's dataset actually is - "BIOS Images" - never MCPX or
    // EEPROM, which Redump does not publish hashes for at all.
    let label = FirmwareSystem::Xbox.redump_dataset_label();
    assert!(label.contains("BIOS Images"));
    assert!(!label.to_ascii_lowercase().contains("mcpx"));
    assert!(!label.to_ascii_lowercase().contains("eeprom"));
}
