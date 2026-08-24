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
    assert_eq!(error, FirmwareEvidenceError::NotPs2BiosDataset);
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
