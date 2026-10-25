//! Synthetic golden headers follow WHDLoad's published structure, independently
//! of the parser implementation: https://whdload.de/docs/autodoc.html
use archivefs_core::identity_source::whdload::parse_whdload_slave;
fn put16(b: &mut [u8], at: usize, n: u16) {
    b[at..at + 2].copy_from_slice(&n.to_be_bytes());
}
fn put32(b: &mut [u8], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_be_bytes());
}
fn golden(version: u16) -> Vec<u8> {
    let mut code = vec![0u8; 256];
    code[..4].copy_from_slice(&[0x70, 0xff, 0x4e, 0x75]);
    code[4..12].copy_from_slice(b"WHDLOADS");
    put16(&mut code, 12, version);
    put32(&mut code, 16, 512 * 1024);
    put16(&mut code, 24, 254); // 16-bit code entry, separate from current directory.
    put16(&mut code, 26, 64);
    put16(&mut code, 28, 72);
    if version >= 4 {
        code[30] = 0x40;
        code[31] = 0x59;
    }
    if version >= 8 {
        put32(&mut code, 32, 512 * 1024);
    }
    if version >= 10 {
        for (at, target) in [(36, 88), (38, 104), (40, 136)] {
            put16(&mut code, at, target);
        }
    }
    if version >= 16 {
        put16(&mut code, 42, 176);
        put32(&mut code, 44, 512 * 1024);
        put16(&mut code, 48, 0x1234);
    }
    if version >= 17 {
        put16(&mut code, 50, 208);
    }
    for (at, s) in [
        (64, b"Data\0".as_slice()),
        (72, b"#?.nocache\0"),
        (88, b"Game\0"),
        (104, b"1985 Publisher\0"),
        (136, b"First\nSecond\xffThird\0"),
        (176, b"kick34005.A500\0"),
        (208, b"C1:B:Trainer\0"),
    ] {
        code[at..at + s.len()].copy_from_slice(s);
    }
    code[254..].copy_from_slice(&[0x4e, 0x75]);
    let mut out = vec![];
    for n in [0x3f3u32, 0, 1, 0, 0, 64, 0x3e9, 64] {
        out.extend_from_slice(&n.to_be_bytes());
    }
    out.extend(code);
    out.extend_from_slice(&0x3f2u32.to_be_bytes());
    out
}
#[test]
fn published_whdload_header_offsets_and_version_gates() {
    for v in [1, 3, 4, 7, 8, 9, 10, 15, 16, 17, 19, 20] {
        let p = parse_whdload_slave(&golden(v))
            .unwrap_or_else(|e| panic!("Valid published v{v} header refused: {e}"));
        assert_eq!(p.game_loader, 254);
        assert_eq!(p.current_dir.as_deref(), Some("Data"));
        assert_eq!(p.key_debug, if v >= 4 { Some(0x40) } else { None });
        assert_eq!(p.key_exit, if v >= 4 { Some(0x59) } else { None });
        assert_eq!(
            p.exp_mem.map(u32::from),
            if v >= 8 { Some(512 * 1024) } else { None }
        );
        assert_eq!(p.name.as_deref(), if v >= 10 { Some("Game") } else { None });
        assert_eq!(
            p.kick_name.as_deref(),
            if v >= 16 {
                Some("kick34005.A500")
            } else {
                None
            }
        );
        assert_eq!(p.kick_size, if v >= 16 { Some(512 * 1024) } else { None });
        assert_eq!(
            p.config.as_deref(),
            if v >= 17 { Some("C1:B:Trainer") } else { None }
        );
    }
}
#[test]
fn published_whdload_info_formatting_and_full_width_memory_are_preserved() {
    let mut bytes = golden(20);
    put32(&mut bytes, 32 + 32, 0xfff80000);
    let p = parse_whdload_slave(&bytes).unwrap();
    assert_eq!(p.exp_mem.map(u32::from), Some(0xfff80000));
    assert_eq!(p.info.as_deref(), Some("First\nSecond\nThird"));
}

#[test]
fn published_whdload_code_and_string_pointers_stay_bounded() {
    for entry in [0, 2, 53, 255, 256, u16::MAX] {
        let mut b = golden(20);
        put16(&mut b, 32 + 24, entry);
        assert!(
            parse_whdload_slave(&b).is_err(),
            "Invalid code entry {entry} accepted"
        );
    }
    for offset in [26, 28, 36, 38, 40, 42, 50] {
        let mut b = golden(20);
        put16(&mut b, 32 + offset, 20);
        assert!(
            parse_whdload_slave(&b).is_err(),
            "Header-overlapping pointer {offset} accepted"
        );
    }
    let mut b = golden(20);
    b[32 + 64] = b'\n';
    assert!(
        parse_whdload_slave(&b).is_err(),
        "Info formatting must not leak into pathname fields"
    );
}
#[test]
fn published_whdload_table_modes_remain_explicit_specialist_limitations() {
    let mut b = golden(20);
    put16(&mut b, 32 + 48, u16::MAX);
    assert!(
        format!("{}", parse_whdload_slave(&b).unwrap_err()).contains("UnsupportedKickstartTable")
    );
    let mut b = golden(20);
    put16(&mut b, 32 + 52, 240);
    assert!(
        format!("{}", parse_whdload_slave(&b).unwrap_err())
            .contains("UnsupportedMemoryConfigurationTable")
    );
}
#[test]
fn published_whdload_config_8bit_label_and_dont_cache_pattern_are_metadata() {
    let mut b = golden(20);
    b[32 + 214] = 0xe9;
    let p = parse_whdload_slave(&b).unwrap();
    assert!(p.config.unwrap().contains('\u{e9}'));
    assert_eq!(p.dont_cache.as_deref(), Some("#?.nocache"));
}
