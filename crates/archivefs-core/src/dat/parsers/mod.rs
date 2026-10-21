//! DAT parser dispatch with backward-compatible format detection.
//!
//! A DAT file might be Logiqx XML or ClrMamePro text. This module sniffs the
//! first bytes of a file to decide which parser to use, then delegates.
//!
//! # Why this does not go through `safe_read`/`TrustedRoots`
//!
//! That policy exists to constrain paths EmuWiz derives from *data* - a RomM
//! record's `archivefs_path`, a cheat catalogue's destination - where a hostile
//! or careless source could otherwise steer a read outside the configured source
//! folders.
//!
//! A DAT path is not derived from anything: in Stage 1A it is typed on the
//! command line by the person running the command, and DAT files normally live
//! wherever they were downloaded rather than inside a source folder. Applying
//! trusted-root confinement here would refuse the ordinary case while protecting
//! against nothing the caller did not already choose.
//!
//! This is therefore a deliberate CLI exception, not an oversight. It stops being
//! one the moment a DAT path arrives from configuration, a manifest or any other
//! stored source - a later stage that feeds paths in that way must route them
//! through the same policy the rest of the codebase uses.

use std::path::Path;

use super::limits::DatLimits;
use super::model::{DatEcosystem, DatFormat};
use super::parser::{ParseError, ParseOutcome};

pub mod clrmamepro;
pub mod logiqx;
pub mod mame_listxml;
mod sink;
use sink::EntrySink;

/// Sniffs the given file path and parses it with the appropriate parser.
pub fn parse_dat_file(path: &Path, limits: DatLimits) -> Result<ParseOutcome, ParseError> {
    let mut outcome = parse_with_sink(path, limits, EntrySink::collect())?;
    super::classification::classify_catalogue(&mut outcome.dat);
    Ok(outcome)
}

/// Opens without blocking on a substituted FIFO, then proves the opened
/// object is a regular file. Registered callers also enforce DAT path policy.
pub(crate) fn open_regular_dat_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular DAT file",
        ));
    }
    Ok(file)
}

/// Captures a private, bounded snapshot before parsing. The returned digest
/// names exactly the bytes the parser consumed, even if the configured file is
/// replaced during an audit. No ROM bytes are read here.
fn snapshot_dat_file(
    path: &Path,
    limits: DatLimits,
) -> Result<(tempfile::NamedTempFile, String), ParseError> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    let io_error = |error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    };
    let mut source = open_regular_dat_file(path).map_err(io_error)?;
    let metadata = source.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err(io_error(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular DAT file",
        )));
    }
    let mut snapshot = tempfile::NamedTempFile::new().map_err(io_error)?;
    let mut digest = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = source.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        size = size.saturating_add(count as u64);
        if size > limits.max_file_size {
            return Err(ParseError::FileTooLarge {
                path: path.to_path_buf(),
                size,
                limit: limits.max_file_size,
            });
        }
        snapshot.write_all(&buffer[..count]).map_err(io_error)?;
        digest.update(&buffer[..count]);
    }
    snapshot.flush().map_err(io_error)?;
    Ok((
        snapshot,
        digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    ))
}

/// Parses and classifies the exact captured catalogue bytes.
pub fn parse_dat_file_bound(
    path: &Path,
    limits: DatLimits,
) -> Result<(ParseOutcome, String), ParseError> {
    let (snapshot, digest) = snapshot_dat_file(path, limits)?;
    let mut parsed = parse_dat_file(snapshot.path(), limits)?;
    parsed.dat.source.file_path = path.to_string_lossy().into_owned();
    Ok((parsed, digest))
}

/// Streaming validation over an identical private snapshot; does not retain ROM entries.
pub fn visit_dat_file_bound_raw(
    path: &Path,
    limits: DatLimits,
    visitor: &mut dyn FnMut(super::model::DatGameEntry) -> Result<(), ParseError>,
) -> Result<(DatStreamSummary, String), ParseError> {
    let (snapshot, digest) = snapshot_dat_file(path, limits)?;
    let mut summary = visit_dat_file_raw(snapshot.path(), limits, visitor)?;
    summary.source.file_path = path.to_string_lossy().into_owned();
    Ok((summary, digest))
}

/// Final source metadata and diagnostics from incremental parsing.
#[derive(Debug)]
pub struct DatStreamSummary {
    pub source: super::model::DatSource,
    pub warnings: Vec<super::parser::ParseWarning>,
}

/// Visits entries in source order without retaining the catalogue.
///
/// Entries carry the same raw fields as the format-specific parsers. Content
/// classification is not applied: it depends on final source metadata, which
/// can appear after entries. Identity/matching callers should continue using
/// `parse_dat_file`. This API is for projections and validation.
///
/// The visitor may report progress by counting entries, or return an error to
/// stop parsing. Delivered entries are provisional until this function returns
/// successfully; a malformed tail or visitor failure must not publish a partial
/// generation. Truncation diagnostics retain each parser's existing semantics.
/// Memory is the reader/event buffers, current entry and bounded diagnostics,
/// plus anything the visitor chooses to retain. XML events and individual
/// records can still be large; existing input and field limits apply.
pub fn visit_dat_file_raw(
    path: &Path,
    limits: DatLimits,
    visitor: &mut dyn FnMut(super::model::DatGameEntry) -> Result<(), ParseError>,
) -> Result<DatStreamSummary, ParseError> {
    let outcome = parse_with_sink(path, limits, EntrySink::visit(visitor))?;
    Ok(DatStreamSummary {
        source: outcome.dat.source,
        warnings: outcome.warnings,
    })
}

fn parse_with_sink(
    path: &Path,
    limits: DatLimits,
    sink: EntrySink<'_>,
) -> Result<ParseOutcome, ParseError> {
    let metadata = std::fs::metadata(path).map_err(|error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    if !metadata.is_file() {
        return Err(ParseError::Io {
            path: path.to_path_buf(),
            error: std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a regular file"),
        });
    }
    let size = metadata.len();
    if size == 0 {
        // Empty file: try ClrMamePro (produces empty result), Logiqx would error.
        let outcome = clrmamepro::parse_clrmamepro_with_sink(path, limits, sink)?;
        return Ok(outcome);
    }
    if size > limits.max_file_size {
        return Err(ParseError::FileTooLarge {
            path: path.to_path_buf(),
            size,
            limit: limits.max_file_size,
        });
    }

    let detected = detect_format(path)?;
    let outcome = match detected {
        DatFormat::Logiqx if is_mame_listxml_root(path)? => {
            mame_listxml::parse_mame_listxml_with_sink(path, limits, sink)
        }
        DatFormat::Logiqx => logiqx::parse_logiqx_with_sink(path, limits, sink),
        DatFormat::ClrMamePro => clrmamepro::parse_clrmamepro_with_sink(path, limits, sink),
    }?;
    // ClrMamePro is the fallback format for anything that is not XML, and its
    // parser ignores every line it does not recognise. A file that produced
    // neither a `clrmamepro (` header nor a single `game (` block therefore
    // never showed any sign of being a catalogue: a text document, a ROM, or
    // an unrecognised binary. Reporting that as a valid empty catalogue hides
    // the mistake from the person who chose the file. A real catalogue that
    // declares a header but lists no games is still accepted as empty.
    if outcome.dat.source.format == DatFormat::ClrMamePro
        && outcome.dat.source.entry_count == 0
        && outcome.dat.source.clrmamepro_header.is_none()
    {
        return Err(ParseError::UnknownFormat);
    }
    Ok(outcome)
}

/// Real `mame -listxml` opens with an inline DTD of about 10 KB before the
/// `<mame>` root, so the root is looked for well beyond the 4 KB format sniff.
const MAME_ROOT_SNIFF_BYTES: usize = 64 * 1024;

/// Decides whether a Logiqx-shaped XML file is actually `mame -listxml`
/// output, whose `<mame>` root uses a different element vocabulary
/// (`<machine>`, not `<game>`) than a Logiqx `<datafile>`.
///
/// This inspects the parsed *root element's tag name* only - never a raw
/// substring search - so a document that merely mentions "mame" in a
/// comment, attribute value, or an unrelated tag (`<mameinfo>`) is never
/// misdetected. Only a bounded prefix (`MAME_ROOT_SNIFF_BYTES`) is inspected; a truncated/malformed prefix is treated as "not
/// conclusively MAME listxml" rather than an error, since the real parser
/// (or `parse_logiqx`) still validates the full document afterward.
fn is_mame_listxml_root(path: &Path) -> Result<bool, ParseError> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let mut buf = vec![0u8; MAME_ROOT_SNIFF_BYTES];
    let n = file.read(&mut buf).map_err(|error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    buf.truncate(n);

    let mut reader = quick_xml::Reader::from_reader(buf.as_slice());
    let mut scan_buf = Vec::new();
    loop {
        match reader.read_event_into(&mut scan_buf) {
            Ok(quick_xml::events::Event::Start(start))
            | Ok(quick_xml::events::Event::Empty(start)) => {
                return Ok(start.name().as_ref().eq_ignore_ascii_case(b"mame"));
            }
            Ok(quick_xml::events::Event::Eof) => return Ok(false),
            Ok(_) => {}
            Err(_) => return Ok(false),
        }
        scan_buf.clear();
    }
}

/// Reads the first few KB of a file and decides its DAT format.
///
/// Logiqx XML files start with `<?xml` or `<datafile` or `<!DOCTYPE datafile`.
/// ClrMamePro files start with `clrmamepro (` after optional whitespace.
pub fn detect_format(path: &Path) -> Result<DatFormat, ParseError> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let mut buf = vec![0u8; 4096];
    let n = file.read(&mut buf).map_err(|error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let mut head = &buf[..n];
    // A UTF-8 BOM (some real TOSEC DATs carry one) precedes the XML
    // declaration and is not Unicode whitespace, so `.trim()` alone never
    // removes it - left unstripped, sniffing below silently fails to
    // recognize `<?xml` and misclassifies a real Logiqx file as
    // ClrMamePro (which then parses zero games, no error surfaced).
    if let Some(without_bom) = head.strip_prefix(b"\xEF\xBB\xBF") {
        head = without_bom;
    }

    // A compressed container is the most common thing a person points at by
    // mistake: DAT-o-MATIC and Redump both deliver a DAT inside a ZIP. The
    // ClrMamePro fallback below would read those bytes as text, find no
    // `game (` block and return an empty catalogue as a success, so the file
    // would be reported as valid with zero entries. Refuse with the reason
    // and what to do about it instead.
    if let Some(container) = archive_container_kind(head) {
        return Err(ParseError::UnsupportedFormat {
            detail: format!(
                "{} is a {container}, not a DAT catalogue. Extract the .dat or .xml file \
                 inside it and choose that file; a No-Intro DAT-o-MATIC download can be \
                 imported as a No-Intro pack ZIP instead.",
                path.display()
            ),
        });
    }

    let trimmed = String::from_utf8_lossy(head).trim().to_ascii_lowercase();

    if trimmed.is_empty() {
        return Ok(DatFormat::ClrMamePro);
    }

    // XML detection: look for XML declaration, datafile root, or DOCTYPE
    if trimmed.starts_with("<?xml")
        || trimmed.starts_with("<datafile")
        || trimmed.starts_with("<!doctype")
    {
        return Ok(DatFormat::Logiqx);
    }

    // ClrMamePro detection
    if trimmed.starts_with("clrmamepro") {
        return Ok(DatFormat::ClrMamePro);
    }

    // Fallback: check if first non-whitespace char is '<'
    if trimmed.starts_with('<') {
        return Ok(DatFormat::Logiqx);
    }

    // Assume ClrMamePro for anything else
    Ok(DatFormat::ClrMamePro)
}

/// Names the compressed container a file's leading bytes identify, if any.
///
/// Magic numbers only; nothing is decompressed or listed. Each signature is
/// the one its format's specification fixes at offset zero, so a text DAT can
/// never start with one of them.
pub(crate) fn archive_container_kind(head: &[u8]) -> Option<&'static str> {
    const SIGNATURES: [(&[u8], &str); 7] = [
        (b"PK\x03\x04", "ZIP archive"),
        (b"PK\x05\x06", "ZIP archive"),
        (b"PK\x07\x08", "ZIP archive"),
        (b"\x1f\x8b", "gzip archive"),
        (b"7z\xBC\xAF\x27\x1C", "7-Zip archive"),
        (b"Rar!\x1A\x07", "RAR archive"),
        (b"\xFD7zXZ\x00", "xz archive"),
    ];
    SIGNATURES
        .iter()
        .find(|(signature, _)| head.starts_with(signature))
        .map(|(_, kind)| *kind)
}

/// The No-Intro, Redump and TOSEC publisher identity declared by a DAT's own
/// header fields, shared by every text format the parsers accept.
///
/// Ecosystem identity is declared by the DAT, never inferred from its
/// filename. Publishers vary which header field carries their name, so every
/// standard internal text field a format preserves is inspected. `None` means
/// the header names none of these three publishers; the caller decides what
/// its format's remaining evidence (software-list roots, FBNeo and MAME
/// branding) says.
pub(super) fn detect_publisher_ecosystem(
    name: Option<&str>,
    author: Option<&str>,
    description: Option<&str>,
    version: Option<&str>,
    homepage: Option<&str>,
    url: Option<&str>,
) -> Option<DatEcosystem> {
    let fields = [name, author, description, version];
    // Substring match, not whole-word: real DAT fields commonly carry
    // these as part of a larger token (e.g. `<author>Redump.org</author>`),
    // unlike the stricter, whole-value No-Intro checks below, which need
    // exactness precisely because "no-intro" is common as a substring of
    // unrelated text and already has its own dedicated logic.
    let contains = |needle: &str| {
        fields
            .iter()
            .flatten()
            .any(|field| field.to_ascii_lowercase().contains(needle))
    };
    let official_no_intro_url = url.is_some_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        ["http://www.no-intro.org", "https://www.no-intro.org"]
            .iter()
            .any(|prefix| value == *prefix || value == format!("{prefix}/"))
    });
    let official_no_intro_homepage =
        homepage.is_some_and(|value| value.trim().eq_ignore_ascii_case("no-intro"));
    let legacy_no_intro_marker = fields.iter().flatten().any(|field| {
        let value = field.trim();
        value.eq_ignore_ascii_case("No-Intro")
            || value.eq_ignore_ascii_case("No-Intro Team")
            || value
                .strip_prefix("No-Intro ")
                .is_some_and(|rest| !rest.is_empty())
    });

    if (official_no_intro_homepage && official_no_intro_url) || legacy_no_intro_marker {
        return Some(DatEcosystem::NoIntro);
    }
    if contains("redump") {
        return Some(DatEcosystem::Redump);
    }
    if contains("tosec") {
        return Some(DatEcosystem::Tosec);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dat::classification::DatContentClass;

    #[test]
    fn detect_logiqx_by_xml_declaration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dat");
        std::fs::write(&path, r#"<?xml version="1.0" encoding="UTF-8"?>"#).unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::Logiqx);
    }

    #[test]
    fn detect_logiqx_by_doctype() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dat");
        std::fs::write(
            &path,
            r#"<!DOCTYPE datafile PUBLIC "-//Logiqx//DTD ROM Management Datafile//EN">"#,
        )
        .unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::Logiqx);
    }

    #[test]
    fn detect_clrmamepro_by_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.dat");
        std::fs::write(&path, "clrmamepro (\n\tname TOSEC\n)\n").unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::ClrMamePro);
    }

    #[test]
    fn detect_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.dat");
        std::fs::write(&path, "").unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::ClrMamePro);
    }

    #[test]
    fn sanitized_no_intro_shape_uses_structured_category_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-intro.dat");
        std::fs::write(
            &path,
            r#"<datafile><header><name>No-Intro Example</name></header><game name="Example" category="Games"><rom name="example.bin" size="4" sha1="a94a8fe5ccb19ba61c4c0873d391e987982fbbd3"/></game></datafile>"#,
        )
        .unwrap();
        let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
        assert_eq!(
            outcome.dat.games[0].content_classification.class,
            DatContentClass::Game
        );
        assert_eq!(
            outcome.dat.games[0].original_metadata.fields["category"],
            "Games"
        );
    }

    #[test]
    fn sanitized_redump_shape_without_category_remains_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("redump.dat");
        std::fs::write(
            &path,
            r#"<datafile><header><name>Redump Example</name></header><game name="Retail Disc"><rom name="disc.bin" size="4" sha1="a94a8fe5ccb19ba61c4c0873d391e987982fbbd3"/></game></datafile>"#,
        )
        .unwrap();
        let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
        assert_eq!(
            outcome.dat.games[0].content_classification.class,
            DatContentClass::Unknown
        );
    }

    #[test]
    fn a_utf8_bom_before_the_xml_declaration_still_detects_as_logiqx() {
        // Batch 8: some real TOSEC DATs carry a UTF-8 BOM before `<?xml`;
        // this must still be recognized as Logiqx, not silently
        // misclassified as ClrMamePro (which would then parse zero games
        // without ever surfacing an error).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.dat");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            br#"<?xml version="1.0" encoding="UTF-8"?><datafile><header><name>BOM Example</name></header><game name="Some Game"><rom name="game.bin" size="4" crc="00000000"/></game></datafile>"#,
        );
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::Logiqx);
        let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
        assert_eq!(outcome.dat.games.len(), 1);
        assert_eq!(outcome.dat.games[0].name, "Some Game");
    }

    #[test]
    fn no_bom_logiqx_detection_is_unaffected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-bom.dat");
        std::fs::write(
            &path,
            r#"<?xml version="1.0"?><datafile><header><name>No BOM</name></header></datafile>"#,
        )
        .unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::Logiqx);
    }

    #[test]
    fn sanitized_tosec_shape_classifies_from_the_set_category() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tosec.dat");
        std::fs::write(
            &path,
            "clrmamepro (\n name \"TOSEC - Commodore Amiga - Games - ADF\"\n)\ngame (\n name \"Quest (Disk 1 of 2)\"\n rom ( name \"quest.adf\" size 4 crc 00000000 )\n)\n",
        )
        .unwrap();
        let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
        assert_eq!(
            outcome.dat.games[0].content_classification.class,
            DatContentClass::RequiredMultidiscPart
        );
    }

    // ------------------------------------------------------------------
    // Batch 9: further DAT format regression (section 7)
    // ------------------------------------------------------------------

    #[test]
    fn bom_before_doctype_still_detects_as_logiqx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom-doctype.dat");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            br#"<!DOCTYPE datafile PUBLIC "-//Logiqx//DTD ROM Management Datafile//EN" "http://www.logiqx.com/Dats/datafile.dtd"><datafile><header><name>BOM DOCTYPE</name></header></datafile>"#,
        );
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::Logiqx);
    }

    #[test]
    fn bom_prefixed_clrmamepro_is_never_misdetected_as_logiqx() {
        // A BOM should never flip detection the *other* direction either -
        // a ClrMamePro file with a leading BOM (unusual, but this test
        // proves the fix is symmetric) still detects as ClrMamePro.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom-clrmamepro.dat");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"clrmamepro (\n name \"Test\"\n)\n");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::ClrMamePro);
    }

    #[test]
    fn whitespace_before_xml_declaration_still_detects_as_logiqx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("whitespace.dat");
        std::fs::write(
            &path,
            "   \n\t<?xml version=\"1.0\"?><datafile><header><name>Whitespace</name></header></datafile>",
        )
        .unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::Logiqx);
    }

    #[test]
    fn bom_and_leading_whitespace_together_still_detect_as_logiqx() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom-whitespace.dat");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(b"  <?xml version=\"1.0\"?><datafile></datafile>");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::Logiqx);
    }

    #[test]
    fn plain_clrmamepro_without_bom_is_unaffected_by_the_bom_fix() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain-cmp.dat");
        std::fs::write(&path, "clrmamepro (\n name \"Plain\"\n)\n").unwrap();
        assert_eq!(detect_format(&path).unwrap(), DatFormat::ClrMamePro);
    }

    #[test]
    fn empty_file_with_only_a_bom_is_not_misdetected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom-only.dat");
        std::fs::write(&path, [0xEFu8, 0xBB, 0xBF]).unwrap();
        // A bare BOM with nothing else is neither a real Logiqx nor
        // ClrMamePro shape - detect_format falls back to ClrMamePro (the
        // same "empty content" default it already used before this
        // milestone), never panics.
        assert_eq!(detect_format(&path).unwrap(), DatFormat::ClrMamePro);
    }

    #[test]
    fn bom_stripping_never_corrupts_real_game_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom-games.dat");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(
            br#"<?xml version="1.0"?><datafile><header><name>BOM Games</name></header><game name="Game One"><rom name="one.bin" size="4" crc="00000000"/></game><game name="Game Two"><rom name="two.bin" size="4" crc="11111111"/></game></datafile>"#,
        );
        std::fs::write(&path, &bytes).unwrap();
        let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
        assert_eq!(outcome.dat.games.len(), 2);
        assert_eq!(outcome.dat.games[0].name, "Game One");
        assert_eq!(outcome.dat.games[1].name, "Game Two");
    }

    fn zip_with_one_dat() -> Vec<u8> {
        use std::io::Write;
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            writer
                .start_file(
                    "Nintendo - Game Boy Color (20261001-000000).dat",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer
                .write_all(br#"<?xml version="1.0"?><datafile><header><name>Nintendo - Game Boy Color</name><author>No-Intro</author></header><game name="A"><rom name="a.gbc" size="1" crc="00000000"/></game></datafile>"#)
                .unwrap();
            writer.finish().unwrap();
        }
        cursor.into_inner()
    }

    /// A DAT-o-MATIC download is a ZIP. Chosen as a DAT it used to fall into
    /// the ClrMamePro fallback and come back as a valid, empty catalogue.
    #[test]
    fn a_zip_chosen_as_a_dat_is_refused_with_the_container_named() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["download.zip", "renamed.dat", "renamed.xml"] {
            let path = dir.path().join(name);
            std::fs::write(&path, zip_with_one_dat()).unwrap();
            let error = parse_dat_file(&path, DatLimits::default()).unwrap_err();
            let message = error.to_string();
            assert!(
                matches!(error, ParseError::UnsupportedFormat { .. }),
                "{name}: {message}"
            );
            assert!(message.contains("ZIP archive"), "{message}");
            assert!(message.contains("Extract the .dat"), "{message}");
            assert!(message.contains("No-Intro pack"), "{message}");
            assert!(
                matches!(
                    detect_format(&path),
                    Err(ParseError::UnsupportedFormat { .. })
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn gzip_sevenzip_rar_and_xz_containers_are_refused_by_signature() {
        let dir = tempfile::tempdir().unwrap();
        for (bytes, kind) in [
            (
                &b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03rest"[..],
                "gzip archive",
            ),
            (&b"7z\xBC\xAF\x27\x1C\x00\x04rest"[..], "7-Zip archive"),
            (&b"Rar!\x1A\x07\x01\x00rest"[..], "RAR archive"),
            (&b"\xFD7zXZ\x00\x00\x04rest"[..], "xz archive"),
        ] {
            let path = dir.path().join("catalogue.dat");
            std::fs::write(&path, bytes).unwrap();
            let error = parse_dat_file(&path, DatLimits::default()).unwrap_err();
            assert!(error.to_string().contains(kind), "{kind}: {error}");
        }
    }

    /// Bytes that are neither XML nor ClrMamePro - a ROM, a text note, a
    /// binary with no known container signature - are not a catalogue, and
    /// saying so beats "0 entries, valid".
    #[test]
    fn content_with_no_catalogue_shape_is_not_a_valid_empty_catalogue() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            (
                "notes.dat",
                &b"These are my notes about ROM sets.\nNothing to see.\n"[..],
            ),
            (
                "rom.dat",
                &[0x00u8, 0xC3, 0x50, 0x01, 0xFF, 0x13, 0x7E, 0x0A, 0x00][..],
            ),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let error = parse_dat_file(&path, DatLimits::default()).unwrap_err();
            assert!(
                matches!(error, ParseError::UnknownFormat),
                "{name}: {error}"
            );
            assert!(
                error
                    .to_string()
                    .contains("not recognised as a DAT catalogue")
            );
        }
    }

    #[test]
    fn a_clrmamepro_catalogue_with_a_header_but_no_games_is_still_an_empty_catalogue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.dat");
        std::fs::write(&path, "clrmamepro (\n\tname \"Empty\"\n)\n").unwrap();
        let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
        assert_eq!(outcome.dat.source.entry_count, 0);
        assert_eq!(outcome.dat.source.name.as_deref(), Some("Empty"));
    }

    #[test]
    fn a_headerless_clrmamepro_catalogue_with_games_still_parses() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("headerless.dat");
        std::fs::write(
            &path,
            "game (\n\tname \"Game\"\n\trom ( name game.bin size 1 crc 00000000 )\n)\n",
        )
        .unwrap();
        let outcome = parse_dat_file(&path, DatLimits::default()).unwrap();
        assert_eq!(outcome.dat.source.entry_count, 1);
    }

    #[test]
    fn publisher_ecosystem_is_shared_between_text_formats() {
        assert_eq!(
            detect_publisher_ecosystem(
                Some("Nintendo - Game Boy"),
                None,
                None,
                None,
                Some("No-Intro"),
                Some("https://www.no-intro.org"),
            ),
            Some(DatEcosystem::NoIntro)
        );
        assert_eq!(
            detect_publisher_ecosystem(None, Some("No-Intro"), None, None, None, None),
            Some(DatEcosystem::NoIntro)
        );
        assert_eq!(
            detect_publisher_ecosystem(None, Some("redump.org"), None, None, None, None),
            Some(DatEcosystem::Redump)
        );
        assert_eq!(
            detect_publisher_ecosystem(
                Some("Sinclair ZX Spectrum - Games - [TAP]"),
                None,
                Some("... (TOSEC-v2026-01-01_CM)"),
                None,
                None,
                None,
            ),
            Some(DatEcosystem::Tosec)
        );
        // A homepage alone, or a non-official URL, is not No-Intro.
        assert_eq!(
            detect_publisher_ecosystem(None, None, None, None, Some("No-Intro"), None),
            None
        );
        assert_eq!(
            detect_publisher_ecosystem(
                Some("Mentions no-intro in passing"),
                None,
                None,
                None,
                None,
                None
            ),
            None
        );
    }
}

#[cfg(test)]
mod bound_tests;
