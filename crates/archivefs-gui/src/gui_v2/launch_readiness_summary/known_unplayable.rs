//! Files that no check can make playable, said at once on Blue's readiness card.
//!
//! A large Xbox ZIP sat on "Checking", then on "File check needed - rescan",
//! and a PS3 `.pkg` on "System needed - Try anyway". None of that is the
//! reason: xemu cannot start a disc image from inside an archive, an Xbox 360
//! disc is not an original Xbox game at all, and a `.pkg` is an installer.
//! Hashing or rescanning the file changes none of it.
//!
//! This reads only the file's own cheap evidence: a ZIP's index (never its
//! contents) and the first bytes of a `.pkg`. It is applied only to a game
//! that is not ready, so a launchable file keeps every normal check, and it
//! never offers a rescan or a launch.

use super::diagnosis::{Cause, Finding};
use super::{GameReadinessSummary, ReadinessPresentationState as State};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A redump-sized original Xbox disc image.
const XBOX_DISC_BYTES: u64 = 7_825_162_240;
/// Full-size Xbox 360 disc images (two XGD2 layouts and XGD3).
const XBOX_360_DISC_BYTES: [u64; 3] = [7_835_492_352, 7_838_695_424, 8_738_846_720];
/// An archive with more entries than this is not a single zipped disc.
const MAX_ZIP_ENTRIES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KnownUnplayable {
    /// An original Xbox disc image inside a ZIP.
    XboxDiscInZip,
    /// A disc image the size of an Xbox 360 disc, inside a ZIP.
    Xbox360DiscInZip,
    /// A PlayStation 3 install package.
    Ps3Package,
    /// A PSP or PS Vita install package.
    PspVitaPackage,
}

fn disc_kind(bytes: u64) -> Option<KnownUnplayable> {
    if bytes == XBOX_DISC_BYTES {
        Some(KnownUnplayable::XboxDiscInZip)
    } else if XBOX_360_DISC_BYTES.contains(&bytes) {
        Some(KnownUnplayable::Xbox360DiscInZip)
    } else {
        None
    }
}

fn has_extension(path: &Path, wanted: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
}

/// The index at the end of a ZIP is at most this large here; a longer one is
/// not a single zipped disc.
const MAX_ZIP_INDEX_BYTES: u64 = 256 * 1024;
/// The end record with the longest allowed comment, plus the ZIP64 locator.
const ZIP_TAIL_BYTES: u64 = 22 + 65_535 + 20;

fn le32(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32::from_le_bytes(
        bytes.get(at..at + 4)?.try_into().ok()?,
    )))
}

fn le64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn le16(bytes: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_le_bytes(
        bytes.get(at..at + 2)?.try_into().ok()?,
    )))
}

fn read_at(file: &mut File, offset: u64, length: u64) -> Option<Vec<u8>> {
    let mut bytes = vec![0_u8; usize::try_from(length).ok()?];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut bytes).ok()?;
    Some(bytes)
}

/// Each member's name and uncompressed size, from the ZIP's index alone: the
/// file's tail, then the index it points to (ZIP64 included). At most three
/// small reads, whatever the archive's size - the general ZIP reader took
/// about fifty seconds on an 8 GB archive on a USB drive.
fn zip_index(path: &Path) -> Option<Vec<(String, u64)>> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let tail_start = length.saturating_sub(ZIP_TAIL_BYTES);
    let tail = read_at(&mut file, tail_start, length - tail_start)?;
    let end = tail
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")?;
    let mut entries = le16(&tail, end + 10)? as u64;
    let mut index_bytes = le32(&tail, end + 12)?;
    let mut index_offset = le32(&tail, end + 16)?;
    if entries == 0xffff || index_bytes == 0xffff_ffff || index_offset == 0xffff_ffff {
        let locator = end.checked_sub(20)?;
        if tail.get(locator..locator + 4)? != b"PK\x06\x07" {
            return None;
        }
        let record = read_at(&mut file, le64(&tail, locator + 8)?, 56)?;
        if record.get(..4)? != b"PK\x06\x06" {
            return None;
        }
        entries = le64(&record, 32)?;
        index_bytes = le64(&record, 40)?;
        index_offset = le64(&record, 48)?;
    }
    if entries > MAX_ZIP_ENTRIES as u64 || index_bytes > MAX_ZIP_INDEX_BYTES {
        return None;
    }
    let index = read_at(&mut file, index_offset, index_bytes)?;
    let mut members = Vec::new();
    let mut at = 0;
    for _ in 0..entries {
        if index.get(at..at + 4)? != b"PK\x01\x02" {
            return None;
        }
        let mut bytes = le32(&index, at + 24)?;
        let (name_length, extra_length, comment_length) = (
            le16(&index, at + 28)?,
            le16(&index, at + 30)?,
            le16(&index, at + 32)?,
        );
        let name = String::from_utf8_lossy(index.get(at + 46..at + 46 + name_length)?).into_owned();
        if bytes == 0xffff_ffff {
            // ZIP64: the real size is the first value of extra field 0x0001.
            let extra = index.get(at + 46 + name_length..at + 46 + name_length + extra_length)?;
            let mut field = 0;
            loop {
                let (id, size) = (le16(extra, field)?, le16(extra, field + 2)?);
                if id == 0x0001 {
                    bytes = le64(extra, field + 4)?;
                    break;
                }
                field += 4 + size;
            }
        }
        members.push((name, bytes));
        at += 46 + name_length + extra_length + comment_length;
    }
    Some(members)
}

/// The file's own answer, or `None` when it proves nothing. Reads a ZIP's
/// index or eight bytes of a package; never the game data.
pub(crate) fn inspect(path: &Path) -> Option<KnownUnplayable> {
    if has_extension(path, "zip") {
        let discs: Vec<u64> = zip_index(path)?
            .into_iter()
            .filter(|(name, _)| has_extension(Path::new(name), "iso"))
            .map(|(_, bytes)| bytes)
            .collect();
        match discs.as_slice() {
            [bytes] => disc_kind(*bytes),
            _ => None,
        }
    } else if has_extension(path, "pkg") {
        let mut header = [0_u8; 8];
        File::open(path).ok()?.read_exact(&mut header).ok()?;
        if header[..4] != [0x7f, b'P', b'K', b'G'] {
            return None;
        }
        match u16::from_be_bytes([header[6], header[7]]) {
            1 => Some(KnownUnplayable::Ps3Package),
            2 => Some(KnownUnplayable::PspVitaPackage),
            _ => None,
        }
    } else {
        None
    }
}

fn words(kind: KnownUnplayable) -> (Cause, &'static str, &'static str) {
    match kind {
        KnownUnplayable::XboxDiscInZip => (
            Cause::FormatUnsupported,
            "xemu needs this disc image taken out of its ZIP",
            "This ZIP holds an original Xbox disc image (.iso). xemu starts a game only from the .iso itself, never from inside an archive. Extract the .iso into your games folder and EmuWiz will list it after that folder's next scan. Checking or rescanning the ZIP will not make it playable. Your file is fine and nothing was changed.",
        ),
        KnownUnplayable::Xbox360DiscInZip => (
            Cause::PlatformUnsupported,
            "This appears to be an Xbox 360 disc, which xemu cannot run",
            "The disc image in this ZIP is the size of an Xbox 360 disc, not an original Xbox one. xemu only emulates the original Xbox, so extracting, converting or rescanning it will not help. Xbox 360 games need an Xbox 360 emulator such as Xenia and belong in an Xbox 360 folder. Your file is fine and nothing was changed.",
        ),
        KnownUnplayable::Ps3Package => (
            Cause::FormatUnsupported,
            "RPCS3 can't start an install package (.pkg) directly",
            "A .pkg file is an installer, not the game itself. Install it once inside RPCS3 (File > Install Packages/Raps/Edats) and start the game from RPCS3's own list. Checking or rescanning the file will not make it playable from here. Your file is fine and nothing was changed.",
        ),
        KnownUnplayable::PspVitaPackage => (
            Cause::PlatformUnsupported,
            "This is a PSP or PS Vita package, not a PS3 game",
            "The file's own header marks it as a PSP / PS Vita install package, so RPCS3 cannot use it and choosing PS3 for it will not help. It belongs with your PS Vita or PSP games, where the emulator for that system installs it. Your file is fine and nothing was changed.",
        ),
    }
}

/// Replaces a not-ready answer with the file's own reason. A ready game is
/// never touched.
pub(crate) fn apply(summary: &mut GameReadinessSummary, kind: KnownUnplayable) {
    if matches!(summary.status, State::Ready | State::ReadyWithWarnings) {
        return;
    }
    let (cause, title, why) = words(kind);
    summary.status = State::Blocked;
    summary.headline = title.to_string();
    summary.explanation = why.to_string();
    summary.primary_action = None;
    summary.warnings.clear();
    summary.findings = vec![Finding {
        cause,
        title,
        why,
        fix: None,
        technical: vec![format!("File evidence: {kind:?}")],
    }];
}

/// [`inspect`] once per file: the answer is kept for the session, keyed by
/// the path with its size and modified time, so the card costs nothing per
/// frame and a replaced file is read again.
pub(crate) fn note_known_unplayable(summary: &mut GameReadinessSummary, path: &Path) {
    if matches!(summary.status, State::Ready | State::ReadyWithWarnings) {
        return;
    }
    if !has_extension(path, "zip") && !has_extension(path, "pkg") {
        return;
    }
    type Key = (PathBuf, u64, Option<std::time::SystemTime>);
    static CACHE: Mutex<Option<HashMap<Key, Option<KnownUnplayable>>>> = Mutex::new(None);
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let key = (path.to_path_buf(), metadata.len(), metadata.modified().ok());
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let known = *cache
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_insert_with(|| inspect(path));
    drop(cache);
    if let Some(kind) = known {
        apply(summary, kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(dir: &Path, name: &str, header: &[u8]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, header).unwrap();
        path
    }

    #[test]
    fn disc_sizes_tell_original_xbox_from_xbox_360() {
        assert_eq!(
            disc_kind(7_825_162_240),
            Some(KnownUnplayable::XboxDiscInZip)
        );
        // Fable Anniversary, Fable III and the Fable II bonus disc.
        for bytes in [8_738_846_720, 7_838_695_424, 7_835_492_352] {
            assert_eq!(disc_kind(bytes), Some(KnownUnplayable::Xbox360DiscInZip));
        }
        assert_eq!(disc_kind(700_000_000), None);
    }

    #[test]
    fn a_package_is_read_by_its_own_header() {
        let dir = tempfile::tempdir().unwrap();
        let ps3 = package(
            dir.path(),
            "a.pkg",
            &[0x7f, b'P', b'K', b'G', 0x80, 0, 0, 1],
        );
        let vita = package(
            dir.path(),
            "b.PKG",
            &[0x7f, b'P', b'K', b'G', 0x80, 0, 0, 2],
        );
        let other = package(dir.path(), "c.pkg", b"not a package");
        let short = package(dir.path(), "d.pkg", &[0x7f, b'P']);
        assert_eq!(inspect(&ps3), Some(KnownUnplayable::Ps3Package));
        assert_eq!(inspect(&vita), Some(KnownUnplayable::PspVitaPackage));
        assert_eq!(inspect(&other), None);
        assert_eq!(inspect(&short), None);
        assert_eq!(inspect(&dir.path().join("missing.pkg")), None);
    }

    #[test]
    fn an_ordinary_zip_proves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        writer
            .start_file("game.iso", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"small").unwrap();
        writer.finish().unwrap();
        assert_eq!(zip_index(&path), Some(vec![("game.iso".to_string(), 5)]));
        assert_eq!(inspect(&path), None);
        let mut summary = super::super::not_started_summary();
        let before = summary.clone();
        note_known_unplayable(&mut summary, &path);
        assert_eq!(summary, before);
    }

    /// The index of an archive like the maintainer's 8 GB ones: one `.iso`
    /// whose size only fits in the ZIP64 fields.
    fn zip64_with_one_disc(path: &Path, disc_bytes: u64) {
        let name = b"Game (Europe).iso";
        let mut extra = vec![0x01, 0x00, 16, 0];
        extra.extend(disc_bytes.to_le_bytes());
        extra.extend(1_000_u64.to_le_bytes());
        let mut entry = b"PK\x01\x02".to_vec();
        entry.extend([0_u8; 16]);
        entry.extend(0xffff_ffff_u32.to_le_bytes());
        entry.extend(0xffff_ffff_u32.to_le_bytes());
        entry.extend((name.len() as u16).to_le_bytes());
        entry.extend((extra.len() as u16).to_le_bytes());
        entry.extend([0_u8; 14]);
        entry.extend(name);
        entry.extend(&extra);
        let data = vec![0_u8; 1_000];
        let index_offset = data.len() as u64;
        let mut record = b"PK\x06\x06".to_vec();
        record.extend(44_u64.to_le_bytes());
        record.extend([0_u8; 12]);
        record.extend(1_u64.to_le_bytes());
        record.extend(1_u64.to_le_bytes());
        record.extend((entry.len() as u64).to_le_bytes());
        record.extend(index_offset.to_le_bytes());
        let mut locator = b"PK\x06\x07".to_vec();
        locator.extend(0_u32.to_le_bytes());
        locator.extend((index_offset + entry.len() as u64).to_le_bytes());
        locator.extend(1_u32.to_le_bytes());
        let mut end = b"PK\x05\x06".to_vec();
        end.extend([0_u8; 4]);
        end.extend(0xffff_u16.to_le_bytes());
        end.extend(0xffff_u16.to_le_bytes());
        end.extend(0xffff_ffff_u32.to_le_bytes());
        end.extend(0xffff_ffff_u32.to_le_bytes());
        end.extend(0_u16.to_le_bytes());
        std::fs::write(path, [data, entry, record, locator, end].concat()).unwrap();
    }

    #[test]
    fn a_zipped_disc_is_recognised_from_the_index_alone() {
        let dir = tempfile::tempdir().unwrap();
        let x360 = dir.path().join("Fable Anniversary.zip");
        zip64_with_one_disc(&x360, 8_738_846_720);
        assert_eq!(
            zip_index(&x360),
            Some(vec![("Game (Europe).iso".to_string(), 8_738_846_720)])
        );
        assert_eq!(inspect(&x360), Some(KnownUnplayable::Xbox360DiscInZip));
        let xbox = dir.path().join("Final Fight.ZIP");
        zip64_with_one_disc(&xbox, 7_825_162_240);
        assert_eq!(inspect(&xbox), Some(KnownUnplayable::XboxDiscInZip));

        let mut summary = super::super::not_started_summary();
        note_known_unplayable(&mut summary, &x360);
        assert_eq!(summary.status, State::Blocked);
        assert!(summary.headline.contains("Xbox 360 disc"));
        // Damaged or cut-off archives prove nothing and are never read further.
        std::fs::write(dir.path().join("cut.zip"), b"PK\x05\x06 short").unwrap();
        assert_eq!(inspect(&dir.path().join("cut.zip")), None);
    }

    #[test]
    fn the_reason_replaces_a_rescan_or_try_anyway_and_never_a_ready_game() {
        for kind in [
            KnownUnplayable::XboxDiscInZip,
            KnownUnplayable::Xbox360DiscInZip,
            KnownUnplayable::Ps3Package,
            KnownUnplayable::PspVitaPackage,
        ] {
            let mut summary = super::super::not_started_summary();
            summary.status = State::NeedsIdentityReview;
            summary.primary_action = Some(super::super::ReadinessAction::CheckGamesFolder);
            apply(&mut summary, kind);
            assert_eq!(summary.status, State::Blocked);
            assert_eq!(summary.primary_action, None, "{kind:?}");
            assert_eq!(summary.findings.len(), 1);
            assert_eq!(summary.findings[0].fix, None);
            assert_eq!(summary.headline, summary.findings[0].title);
            assert!(summary.explanation.contains("nothing was changed"));

            let mut ready = super::super::not_started_summary();
            ready.status = State::Ready;
            let before = ready.clone();
            apply(&mut ready, kind);
            assert_eq!(ready, before, "{kind:?}");
        }
        let (_, title, why) = words(KnownUnplayable::Xbox360DiscInZip);
        assert!(title.contains("Xbox 360"));
        assert!(why.contains("extracting, converting or rescanning it will not help"));
        let (_, _, why) = words(KnownUnplayable::XboxDiscInZip);
        assert!(why.contains("rescanning the ZIP will not make it playable"));
    }

    #[test]
    fn a_package_on_disk_is_explained_through_the_cached_note() {
        let dir = tempfile::tempdir().unwrap();
        let ps3 = package(
            dir.path(),
            "game.pkg",
            &[0x7f, b'P', b'K', b'G', 0, 0, 0, 1],
        );
        let mut summary = super::super::not_started_summary();
        summary.status = State::NeedsSystem;
        note_known_unplayable(&mut summary, &ps3);
        assert_eq!(
            summary.headline,
            "RPCS3 can't start an install package (.pkg) directly"
        );
        assert!(summary.explanation.contains("Install Packages/Raps/Edats"));
    }
}

#[cfg(test)]
mod by_hand {
    /// Read-only and run by hand: `EMUWIZ_PROBE_FILE=/path/to/file scripts/cargo-iso test -p archivefs-gui
    /// --lib known_unplayable::by_hand -- --ignored --nocapture` prints what one real file proves.
    #[test]
    #[ignore = "reads the file named in EMUWIZ_PROBE_FILE; run by hand"]
    fn this_file() {
        let path = std::env::var_os("EMUWIZ_PROBE_FILE").expect("set EMUWIZ_PROBE_FILE");
        let path = std::path::Path::new(&path);
        println!("{:?}", super::inspect(path));
        println!("{:?}", super::zip_index(path));
    }
}
