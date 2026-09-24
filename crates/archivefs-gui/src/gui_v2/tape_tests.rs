use super::pages::native_route_for_handoff;
use super::routes::{Route, Section};

fn zx_tap() -> Vec<u8> {
    let mut payload = vec![0, 3];
    payload.extend_from_slice(b"V2 TAPE   ");
    payload.extend_from_slice(&[4, 0, 0, 0x80, 0, 0]);
    payload.push(payload.iter().fold(0, |sum, byte| sum ^ byte));
    let mut bytes = (payload.len() as u16).to_le_bytes().to_vec();
    bytes.extend(payload);
    bytes
}

fn t64() -> Vec<u8> {
    let mut bytes = vec![0u8; 98];
    bytes[..20].copy_from_slice(b"C64S tape image file");
    bytes[32..34].copy_from_slice(&0x0101u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&1u16.to_le_bytes());
    bytes[36..38].copy_from_slice(&1u16.to_le_bytes());
    bytes[64] = 1;
    bytes[65] = 0x82;
    bytes[66..68].copy_from_slice(&0x0801u16.to_le_bytes());
    bytes[68..70].copy_from_slice(&0x0803u16.to_le_bytes());
    bytes[72..76].copy_from_slice(&96u32.to_le_bytes());
    bytes[80..89].copy_from_slice(b"V2 TAPE  ");
    bytes
}

fn commodore_tap() -> Vec<u8> {
    let mut bytes = b"C64-TAPE-RAW".to_vec();
    bytes.extend([2, 0, 0, 0]);
    bytes.extend(1u32.to_le_bytes());
    bytes.push(32);
    bytes
}

#[test]
fn tape_inspector_is_a_first_class_v2_route() {
    assert_eq!(Section::Tape.title(), "Tape Inspector");
    assert_eq!(
        Route::Task {
            section: Section::Tape,
            game: 42,
        }
        .section(),
        Section::Tape
    );
}

#[test]
fn tape_inspector_does_not_use_the_legacy_handoff() {
    // Tape is rendered by the explicit v2 dispatcher, never by the generic
    // task fallback that can open a second --legacy window.
    assert_eq!(native_route_for_handoff(Section::Tape, Some(42)), None);
}

#[test]
fn v2_tape_route_reuses_all_supported_byte_analysis_formats() {
    for bytes in [
        zx_tap(),
        b"ZXTape!\x1a\x01\x14\x20\xe8\x03".to_vec(),
        t64(),
        commodore_tap(),
    ] {
        assert!(super::super::tape_analysis_page::analyze_bytes(&bytes, None).is_ok());
    }
}

#[test]
fn v2_tape_route_keeps_malformed_and_oversized_inputs_bounded() {
    assert!(super::super::tape_analysis_page::analyze_bytes(b"not a tape", None).is_err());
    let oversized = vec![0u8; archivefs_core::tape_analysis::MAX_ANALYSIS_BYTES + 1];
    assert!(super::super::tape_analysis_page::analyze_bytes(&oversized, None).is_err());
}
