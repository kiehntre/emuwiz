use std::cell::Cell;

use super::fixtures::{Entry, archive, crc16, encode};
use super::*;

fn scan(bytes: &[u8]) -> Result<Vec<LhaRawEntry>, LhaHeaderError> {
    scan_headers(bytes, bytes.len() as u64, 1024)
}

fn kind_of(entry: Entry) -> LhaEntryKind {
    let entries = scan(&archive(&[entry])).unwrap();
    assert_eq!(entries.len(), 1);
    entries[0].kind.clone()
}

#[test]
fn every_header_level_proves_regular_symlink_directory_and_special() {
    for level in 0..=3 {
        let regular = Entry::unix("Game.rom", b"ROM", 0o100644).level(level);
        assert_eq!(kind_of(regular), LhaEntryKind::Regular, "level {level}");
        let symlink = Entry::unix("Game.rom|target", b"target", 0o120777).level(level);
        assert_eq!(kind_of(symlink), LhaEntryKind::Symlink, "level {level}");
        let directory = Entry::directory("Game").level(level);
        assert_eq!(kind_of(directory), LhaEntryKind::Directory, "level {level}");
        for mode in [0o010644, 0o020644, 0o060644, 0o140644] {
            let special = Entry::unix("node", b"", mode).level(level);
            assert_eq!(
                kind_of(special),
                LhaEntryKind::Special,
                "level {level} {mode:o}"
            );
        }
    }
}

#[test]
fn a_symlink_is_a_symlink_even_under_a_directory_method() {
    // lha 1.14-style writers may use the `-lhd-` method for links.
    let mut entry = Entry::unix("a|b", b"", 0o120777);
    entry.method = *b"-lhd-";
    assert_eq!(kind_of(entry), LhaEntryKind::Symlink);
}

#[test]
fn dos_family_hosts_without_a_mode_are_regular_only_when_nothing_contradicts() {
    for host in [0, b'M', b'w', b'W', b'2', b'A'] {
        for level in 0..=3 {
            let mut entry = Entry::file("Game.rom", b"ROM").level(level);
            entry.host = host;
            assert_eq!(
                kind_of(entry),
                LhaEntryKind::Regular,
                "{host} level {level}"
            );
        }
    }
    let mut directory = Entry::file("Dir", b"");
    directory.method = *b"-lhd-";
    assert_eq!(kind_of(directory), LhaEntryKind::Directory);
    let mut volume = Entry::file("LABEL", b"");
    volume.dos_attr = 0x08;
    assert_eq!(kind_of(volume), LhaEntryKind::Special);
    let mut attr_dir = Entry::file("Dir", b"");
    attr_dir.dos_attr = 0x10;
    assert!(matches!(kind_of(attr_dir), LhaEntryKind::Unknown(_)));
}

#[test]
fn unproven_types_are_unknown_never_regular() {
    // Unix host with no mode, unrecognised host, link separator with no mode.
    let mut unix_no_mode = Entry::file("Game.rom", b"ROM");
    unix_no_mode.host = b'U';
    assert!(matches!(kind_of(unix_no_mode), LhaEntryKind::Unknown(_)));
    let mut strange_host = Entry::file("Game.rom", b"ROM");
    strange_host.host = b'Z';
    assert!(matches!(kind_of(strange_host), LhaEntryKind::Unknown(_)));
    assert!(matches!(
        kind_of(Entry::file("a|b", b"x")),
        LhaEntryKind::Unknown(_)
    ));
    // Contradictions between method, mode and attributes.
    let mut regular_lhd = Entry::unix("x", b"", 0o100644);
    regular_lhd.method = *b"-lhd-";
    assert!(matches!(kind_of(regular_lhd), LhaEntryKind::Unknown(_)));
    assert!(matches!(
        kind_of(Entry::unix("x", b"", 0o040755)),
        LhaEntryKind::Unknown(_)
    ));
    let mut attr_conflict = Entry::unix("x", b"R", 0o100644);
    attr_conflict.dos_attr = 0x10;
    assert!(matches!(kind_of(attr_conflict), LhaEntryKind::Unknown(_)));
    // Permission bits only, no file-type bits.
    assert!(matches!(
        kind_of(Entry::unix("x", b"R", 0o644)),
        LhaEntryKind::Unknown(_)
    ));
    // A directory carrying data.
    let mut dir_data = Entry::directory("d");
    dir_data.payload = b"x".to_vec();
    assert!(matches!(kind_of(dir_data), LhaEntryKind::Unknown(_)));
    // A regular '|' name WITH a regular mode is genuinely regular.
    assert_eq!(
        kind_of(Entry::unix("a|b", b"x", 0o100644)),
        LhaEntryKind::Regular
    );
}

#[test]
fn irregular_extended_headers_leave_the_type_unproven() {
    for level in 1..=3 {
        let mut duplicate = Entry::unix("x", b"R", 0o100644).level(level);
        duplicate
            .extra_extended
            .push((0x50, 0o120777_u16.to_le_bytes().to_vec()));
        assert!(
            matches!(kind_of(duplicate), LhaEntryKind::Unknown(_)),
            "duplicate mode, level {level}"
        );
        let mut short = Entry::unix("x", b"R", 0o100644).level(level);
        short.extra_extended.push((0x40, vec![1]));
        assert!(
            matches!(kind_of(short), LhaEntryKind::Unknown(_)),
            "short attribute, level {level}"
        );
    }
}

#[test]
fn header_crc_is_checked_for_levels_two_and_three() {
    for level in [2, 3] {
        let mut bytes = archive(&[Entry::unix("Game.rom", b"ROM", 0o100644).level(level)]);
        let mode_position = bytes
            .windows(2)
            .position(|w| w == 0o100644_u16.to_le_bytes())
            .unwrap();
        bytes[mode_position] ^= 0x01; // flips 0o100644 -> still looks like a mode
        let entries = scan(&bytes).unwrap();
        assert!(
            matches!(entries[0].kind, LhaEntryKind::Unknown(_)),
            "level {level} tampered header must not stay proven"
        );
    }
}

#[test]
fn level_one_trailing_base_bytes_leave_the_type_unproven() {
    let mut bytes = encode(&Entry::unix("x", b"R", 0o100644).level(1));
    let name_len = 1;
    let after_next = 2 + 25 + name_len; // end of base header incl. next-size
    bytes.splice(after_next..after_next, [0xAA, 0xBB]);
    bytes[0] += 2;
    bytes[1] = bytes[2..2 + bytes[0] as usize]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    let entries = scan(&[bytes, vec![0]].concat()).unwrap();
    assert!(matches!(entries[0].kind, LhaEntryKind::Unknown(_)));
}

#[test]
fn directory_names_and_separators_are_assembled_from_headers() {
    // Level 0/1 use 0xFF as the separator; levels 2/3 use the 0x02 header.
    let mut entry = Entry::file("x", b"ROM");
    entry.name = b"Dir\xffGame.rom".to_vec();
    assert_eq!(scan(&archive(&[entry])).unwrap()[0].name, b"Dir/Game.rom");
    for level in [2, 3] {
        let mut entry = Entry::unix("Game.rom", b"ROM", 0o100644).level(level);
        entry
            .extra_extended
            .push((0x02, b"Dir\xffSub\xff".to_vec()));
        let parsed = scan(&archive(&[entry])).unwrap();
        assert_eq!(parsed[0].name, b"Dir/Sub/Game.rom", "level {level}");
    }
    // A duplicate filename header is irregular.
    let mut entry = Entry::unix("a", b"R", 0o100644).level(2);
    entry.extra_extended.push((0x01, b"b".to_vec()));
    assert!(matches!(kind_of(entry), LhaEntryKind::Unknown(_)));
}

#[test]
fn unicode_names_are_preserved_as_exact_bytes() {
    let parsed = scan(&archive(&[Entry::unix(
        "Spiel/Größe-ゲーム.rom",
        b"R",
        0o100644,
    )
    .level(2)]))
    .unwrap();
    assert_eq!(parsed[0].name_utf8(), Some("Spiel/Größe-ゲーム.rom"));
    let mut latin1 = Entry::file("x", b"R");
    latin1.name = b"Gr\xf6\xdfe.rom".to_vec();
    assert_eq!(scan(&archive(&[latin1])).unwrap()[0].name_utf8(), None);
}

#[test]
fn truncated_and_malformed_headers_fail_closed() {
    let good = archive(&[Entry::unix("Game.rom", b"ROMDATA", 0o100644).level(1)]);
    // Every proper prefix that still starts a header is an error, never a
    // shorter "successful" archive.
    for cut in 1..good.len() - 1 {
        let result = scan(&good[..cut]);
        assert!(
            result.is_err(),
            "prefix of {cut} bytes was accepted: {result:?}"
        );
    }
    // Header checksum.
    let mut bad = archive(&[Entry::file("Game.rom", b"ROM")]);
    bad[1] ^= 0xff;
    assert!(matches!(scan(&bad), Err(LhaHeaderError::Malformed { .. })));
    // Invalid level and method framing.
    let mut level = archive(&[Entry::file("Game.rom", b"ROM")]);
    level[20] = 9;
    assert!(matches!(
        scan(&level),
        Err(LhaHeaderError::Malformed { .. })
    ));
    let mut method = archive(&[Entry::file("Game.rom", b"ROM")]);
    method[2] = b'X';
    assert!(matches!(
        scan(&method),
        Err(LhaHeaderError::Malformed { .. })
    ));
    // Packed size larger than the file.
    let mut huge = archive(&[Entry::file("Game.rom", b"ROM")]);
    huge[7..11].copy_from_slice(&u32::MAX.to_le_bytes());
    huge[1] = huge[2..2 + huge[0] as usize]
        .iter()
        .fold(0_u8, |s, b| s.wrapping_add(*b));
    assert!(matches!(scan(&huge), Err(LhaHeaderError::Truncated { .. })));
}

#[test]
fn malformed_extended_header_chains_fail_closed() {
    let base = encode(&Entry::unix("x", b"R", 0o100644).level(1));
    // First extended header claims a size below its own framing.
    let mut tiny = base.clone();
    let first_size_at = 2 + 25;
    tiny[first_size_at..first_size_at + 2].copy_from_slice(&1_u16.to_le_bytes());
    tiny[1] = tiny[2..2 + tiny[0] as usize]
        .iter()
        .fold(0_u8, |s, b| s.wrapping_add(*b));
    assert!(matches!(
        scan(&[tiny, vec![0]].concat()),
        Err(LhaHeaderError::Malformed { .. })
    ));
    // Declared extended size runs past the end of the file.
    let mut overrun = base.clone();
    overrun[first_size_at..first_size_at + 2].copy_from_slice(&0xffff_u16.to_le_bytes());
    overrun[1] = overrun[2..2 + overrun[0] as usize]
        .iter()
        .fold(0_u8, |s, b| s.wrapping_add(*b));
    assert!(scan(&[overrun, vec![0]].concat()).is_err());
    // A chain longer than the header-count bound.
    let mut many = Entry::unix("x", b"R", 0o100644).level(1);
    for _ in 0..MAX_EXTENDED_HEADERS + 5 {
        many.extra_extended.push((0x77, vec![]));
    }
    assert!(matches!(
        scan(&archive(&[many])),
        Err(LhaHeaderError::Malformed { .. })
    ));
    // Level-1 skip size smaller than the extended headers it must contain.
    let mut skip = base.clone();
    skip[7..11].copy_from_slice(&0_u32.to_le_bytes());
    assert!(matches!(
        scan(&[skip, vec![0]].concat()),
        Err(LhaHeaderError::Malformed { .. })
    ));
    // Level-2 chain overrunning its own declared header size.
    let mut level2 = encode(&Entry::unix("x", b"R", 0o100644).level(2));
    let total = u16::from_le_bytes([level2[0], level2[1]]) - 2;
    level2[..2].copy_from_slice(&total.to_le_bytes());
    assert!(scan(&[level2, vec![0]].concat()).is_err());
    // Level-3 header size above the ceiling.
    let mut level3 = encode(&Entry::unix("x", b"R", 0o100644).level(3));
    level3[24..28].copy_from_slice(&(2 * 1024 * 1024_u32).to_le_bytes());
    assert!(matches!(
        scan(&[level3, vec![0]].concat()),
        Err(LhaHeaderError::Malformed { .. })
    ));
}

#[test]
fn entry_count_is_bounded() {
    let entries: Vec<Entry> = (0..10)
        .map(|i| Entry::file(&format!("f{i}"), b"x"))
        .collect();
    let bytes = archive(&entries);
    assert!(matches!(
        scan_headers(bytes.as_slice(), bytes.len() as u64, 9),
        Err(LhaHeaderError::TooManyEntries)
    ));
    assert_eq!(
        scan_headers(bytes.as_slice(), bytes.len() as u64, 10)
            .unwrap()
            .len(),
        10
    );
}

struct Counting<'a> {
    bytes: &'a [u8],
    read: Cell<u64>,
}

impl ReadAt for Counting<'_> {
    fn read_exact_at_offset(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        self.read.set(self.read.get() + buf.len() as u64);
        self.bytes.read_exact_at_offset(buf, offset)
    }
}

#[test]
fn scanning_reads_headers_only_never_member_payloads() {
    // 2,000 members each with a 64 KiB payload (128 MiB of archive).
    let payload = vec![0xA5_u8; 64 * 1024];
    let entries: Vec<Entry> = (0..2000)
        .map(|i| {
            Entry::unix(&format!("Game/part{i:04}.rom"), &payload, 0o100644)
                .level(1 + (i % 3) as u8)
        })
        .collect();
    let bytes = archive(&entries);
    let source = Counting {
        bytes: &bytes,
        read: Cell::new(0),
    };
    let started = std::time::Instant::now();
    let parsed = scan_headers(&source, bytes.len() as u64, 4096).unwrap();
    let elapsed = started.elapsed();
    assert_eq!(parsed.len(), 2000);
    assert!(parsed.iter().all(|entry| entry.kind.is_regular()));
    let read = source.read.get();
    eprintln!(
        "LHA SCAN archive={} bytes headers_read={} bytes ({:.3}%) entries=2000 elapsed={:?}",
        bytes.len(),
        read,
        read as f64 * 100.0 / bytes.len() as f64,
        elapsed
    );
    assert!(
        read < 2000 * 400,
        "read {read} bytes - payload must not be read"
    );
}

#[test]
fn raw_fixture_crc_matches_the_header_declaration() {
    let parsed = scan(&archive(&[Entry::file("Game.rom", b"ROMDATA")])).unwrap();
    assert_eq!(parsed[0].crc16, crc16(b"ROMDATA"));
    assert_eq!(parsed[0].original_size, 7);
    assert_eq!(parsed[0].method_str(), "-lh0-");
}
