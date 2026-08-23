//! Bounded streaming parser for current `mame -listxml` output.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::dat::classification::{DatContentClassification, DatOriginalMetadata};
use crate::dat::hash::{normalise_crc32, normalise_md5, normalise_sha1};
use crate::dat::limits::DatLimits;
use crate::dat::model::{
    DatDeviceRefEntry, DatDiskEntry, DatEcosystem, DatFormat, DatGameEntry, DatRomEntry, DatSource,
    ParsedDat,
};
use crate::dat::parser::{ParseError, ParseOutcome, ParseWarning};

pub fn parse_mame_listxml(path: &Path, limits: DatLimits) -> Result<ParseOutcome, ParseError> {
    let metadata = std::fs::metadata(path).map_err(|error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    if metadata.len() > limits.max_file_size {
        return Err(ParseError::FileTooLarge {
            path: path.to_path_buf(),
            size: metadata.len(),
            limit: limits.max_file_size,
        });
    }
    let file = File::open(path).map_err(|error| ParseError::Io {
        path: path.to_path_buf(),
        error,
    })?;
    let mut reader = Reader::from_reader(BufReader::with_capacity(64 * 1024, file));
    let mut warnings = Vec::new();
    let mut games = Vec::new();
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut in_machine = false;
    let mut text = String::new();
    let mut current: Option<Machine> = None;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > limits.max_xml_depth {
                    return Err(ParseError::XmlDepthExceeded {
                        depth,
                        limit: limits.max_xml_depth,
                    });
                }
                let tag = tag(&e)?;
                if tag == "machine" {
                    if games.len() >= limits.max_entries {
                        return Err(ParseError::EntryLimitExceeded {
                            count: games.len(),
                            limit: limits.max_entries,
                        });
                    }
                    current = Some(Machine::from_attrs(&e, &mut warnings, limits.max_warnings));
                    in_machine = true;
                } else if in_machine {
                    handle_empty_like(&tag, &e, current.as_mut().unwrap(), &limits, &mut warnings)?;
                }
                text.clear();
            }
            Ok(Event::Empty(e)) => {
                let tag = tag(&e)?;
                if tag == "machine" {
                    if games.len() >= limits.max_entries {
                        return Err(ParseError::EntryLimitExceeded {
                            count: games.len(),
                            limit: limits.max_entries,
                        });
                    }
                    if let Some(game) =
                        Machine::from_attrs(&e, &mut warnings, limits.max_warnings).finish()
                    {
                        games.push(game);
                    } else {
                        warn(
                            &mut warnings,
                            limits.max_warnings,
                            "machine_missing_name",
                            "MAME machine without name skipped",
                        );
                    }
                } else if in_machine {
                    handle_empty_like(&tag, &e, current.as_mut().unwrap(), &limits, &mut warnings)?;
                }
            }
            Ok(Event::Text(e)) => {
                if in_machine {
                    text.push_str(&e.decode().map_err(xml_error)?);
                }
            }
            Ok(Event::End(e)) => {
                let tag = end_tag(&e)?;
                if tag == "machine" {
                    in_machine = false;
                    if let Some(machine) = current.take() {
                        if let Some(game) = machine.finish() {
                            games.push(game)
                        } else {
                            warn(
                                &mut warnings,
                                limits.max_warnings,
                                "machine_missing_name",
                                "MAME machine without name skipped",
                            )
                        }
                    }
                } else if in_machine {
                    let machine = current.as_mut().unwrap();
                    let value = text.trim();
                    if !value.is_empty() {
                        match tag.as_str() {
                            "description" => {
                                machine.description =
                                    Some(bounded(value, limits.max_description_length)?)
                            }
                            "year" => {
                                machine.year = Some(bounded(value, limits.max_description_length)?)
                            }
                            "manufacturer" => {
                                machine.manufacturer =
                                    Some(bounded(value, limits.max_description_length)?)
                            }
                            _ => {}
                        }
                    }
                }
                depth = depth.saturating_sub(1);
                text.clear();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => {
                return Err(ParseError::MalformedXml {
                    detail: e.to_string(),
                    byte_offset: Some(reader.buffer_position() as usize),
                });
            }
        };
        buf.clear();
    }
    Ok(ParseOutcome {
        dat: ParsedDat {
            source: DatSource {
                format: DatFormat::Logiqx,
                ecosystem: DatEcosystem::MAMEArcade,
                file_path: path.to_string_lossy().into_owned(),
                name: Some("MAME -listxml".into()),
                description: None,
                version: None,
                author: None,
                homepage: None,
                clrmamepro_header: None,
                entry_count: games.len(),
                rom_count: games.iter().map(|g| g.roms.len()).sum(),
                parse_warnings: warnings.into_iter().map(|w| w.to_string()).collect(),
            },
            games,
        },
        warnings: Vec::new(),
    })
}

fn xml_error(e: quick_xml::encoding::EncodingError) -> ParseError {
    ParseError::MalformedXml {
        detail: e.to_string(),
        byte_offset: None,
    }
}
fn tag(e: &quick_xml::events::BytesStart<'_>) -> Result<String, ParseError> {
    std::str::from_utf8(e.name().as_ref())
        .map(|s| s.to_ascii_lowercase())
        .map_err(|e| ParseError::MalformedXml {
            detail: e.to_string(),
            byte_offset: None,
        })
}
fn end_tag(e: &quick_xml::events::BytesEnd<'_>) -> Result<String, ParseError> {
    std::str::from_utf8(e.name().as_ref())
        .map(|s| s.to_ascii_lowercase())
        .map_err(|e| ParseError::MalformedXml {
            detail: e.to_string(),
            byte_offset: None,
        })
}
fn attr(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.try_get_attribute(name)
        .ok()
        .flatten()
        .and_then(|a| a.normalized_value(quick_xml::XmlVersion::Implicit1_0).ok())
        .map(|v| v.into_owned())
        .filter(|v| !v.is_empty())
}
fn bounded(value: &str, max: usize) -> Result<String, ParseError> {
    if value.len() > max {
        Err(ParseError::DescriptionTooLong {
            length: value.len(),
            limit: max,
        })
    } else {
        Ok(value.to_string())
    }
}
fn warn(warnings: &mut Vec<ParseWarning>, max: usize, code: &'static str, msg: &str) {
    if warnings.len() < max {
        warnings.push(ParseWarning::with_code(code, msg));
    }
}

#[derive(Default)]
struct Machine {
    name: Option<String>,
    description: Option<String>,
    year: Option<String>,
    manufacturer: Option<String>,
    clone_of: Option<String>,
    rom_of: Option<String>,
    sample_of: Option<String>,
    is_bios: Option<String>,
    is_device: Option<String>,
    runnable: Option<String>,
    source_file: Option<String>,
    metadata: DatOriginalMetadata,
    roms: Vec<DatRomEntry>,
    disks: Vec<DatDiskEntry>,
    device_refs: Vec<DatDeviceRefEntry>,
}
impl Machine {
    fn from_attrs(
        e: &quick_xml::events::BytesStart<'_>,
        _: &mut Vec<ParseWarning>,
        _: usize,
    ) -> Self {
        let mut m = Self {
            name: attr(e, b"name"),
            clone_of: attr(e, b"cloneof"),
            rom_of: attr(e, b"romof"),
            sample_of: attr(e, b"sampleof"),
            is_bios: attr(e, b"isbios"),
            is_device: attr(e, b"isdevice"),
            runnable: attr(e, b"runnable"),
            source_file: attr(e, b"sourcefile"),
            ..Default::default()
        };
        for key in [b"ismechanical".as_slice(), b"sampleof".as_slice()] {
            if let Some(v) = attr(e, key) {
                m.metadata
                    .fields
                    .insert(String::from_utf8_lossy(key).into_owned(), v);
            }
        }
        m
    }
    fn finish(self) -> Option<DatGameEntry> {
        Some(DatGameEntry {
            name: self.name?,
            description: self.description,
            roms: self.roms,
            clone_of: self.clone_of,
            rom_of: self.rom_of,
            sample_of: self.sample_of,
            is_bios: self.is_bios,
            is_device: self.is_device,
            runnable: self.runnable,
            disks: self.disks,
            device_refs: self.device_refs,
            year: self.year,
            manufacturer: self.manufacturer,
            source_file: self.source_file,
            original_metadata: self.metadata,
            content_classification: DatContentClassification::unknown(),
            ..Default::default()
        })
    }
}
fn handle_empty_like(
    tag: &str,
    e: &quick_xml::events::BytesStart<'_>,
    m: &mut Machine,
    limits: &DatLimits,
    warnings: &mut Vec<ParseWarning>,
) -> Result<(), ParseError> {
    match tag {
        "rom" => {
            if m.roms.len() >= limits.max_roms_per_entry {
                return Err(ParseError::RomsPerEntryExceeded {
                    game_name: m.name.clone().unwrap_or_else(|| "<unnamed machine>".into()),
                    count: m.roms.len(),
                    limit: limits.max_roms_per_entry,
                });
            }
            let status = attr(e, b"status");
            let crc = attr(e, b"crc").and_then(|v| normalise_crc32(&v));
            let md5 = attr(e, b"md5").and_then(|v| normalise_md5(&v));
            let sha1 = attr(e, b"sha1").and_then(|v| normalise_sha1(&v));
            if status
                .as_deref()
                .is_some_and(|v| v.eq_ignore_ascii_case("nodump"))
                && (crc.is_some() || md5.is_some() || sha1.is_some())
            {
                warn(
                    warnings,
                    limits.max_warnings,
                    "nodump_with_hash",
                    "ROM declares status=nodump together with a checksum",
                );
            }
            m.roms.push(DatRomEntry {
                name: attr(e, b"name").unwrap_or_default(),
                size_bytes: attr(e, b"size").and_then(|v| v.parse().ok()),
                crc32: crc,
                md5,
                sha1,
                status,
                merge: attr(e, b"merge"),
                ..Default::default()
            });
        }
        "disk" => m.disks.push(DatDiskEntry {
            name: attr(e, b"name"),
            sha1: attr(e, b"sha1").and_then(|v| normalise_sha1(&v)),
            status: attr(e, b"status"),
            merge: attr(e, b"merge"),
            ..Default::default()
        }),
        "device_ref" => m.device_refs.push(DatDeviceRefEntry {
            name: attr(e, b"name"),
        }),
        "softwarelist" => {
            if let Some(name) = attr(e, b"name") {
                m.metadata.fields.insert(
                    format!("softwarelist.{name}"),
                    attr(e, b"status").unwrap_or_default(),
                );
            }
        }
        "driver" => {
            if let Some(status) = attr(e, b"status") {
                m.metadata.fields.insert("driver.status".into(), status);
            }
        }
        "slot" | "slotoption" | "chip" | "display" | "sound" | "input" | "dipswitch" => {
            let key = format!("mame.{tag}");
            m.metadata
                .fields
                .entry(key)
                .or_insert_with(|| attr(e, b"name").unwrap_or_default());
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &std::path::Path, name: &str, contents: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_machine_attributes_and_children() {
        let dir = tempfile::tempdir().unwrap();
        let xml = r#"<mame build="0.280"><machine name="pacman" cloneof="puckman" romof="puckman" sampleof="pacman" isbios="no" isdevice="no" runnable="yes" sourcefile="pacman.cpp"><description>Pac-Man</description><year>1980</year><manufacturer>Namco</manufacturer><rom name="pacman.6e" size="4096" crc="c1e6ab10" sha1="e87e059c5be45753f7e9f17dc8d1d3b96ff8fe0d"/><disk name="pacman" sha1="0000000000000000000000000000000000000a"/><device_ref name="z80"/></machine></mame>"#;
        let path = write(dir.path(), "pacman.xml", xml);
        let outcome = parse_mame_listxml(&path, DatLimits::default()).unwrap();
        assert_eq!(outcome.dat.source.ecosystem, DatEcosystem::MAMEArcade);
        assert_eq!(outcome.dat.games.len(), 1);
        let game = &outcome.dat.games[0];
        assert_eq!(game.name, "pacman");
        assert_eq!(game.clone_of.as_deref(), Some("puckman"));
        assert_eq!(game.rom_of.as_deref(), Some("puckman"));
        assert_eq!(game.runnable.as_deref(), Some("yes"));
        assert_eq!(game.roms.len(), 1);
        assert_eq!(game.disks.len(), 1);
        assert_eq!(game.device_refs.len(), 1);
        assert_eq!(game.roms[0].crc32.as_deref(), Some("c1e6ab10"));
    }

    #[test]
    fn machine_without_name_is_skipped_with_warning() {
        let dir = tempfile::tempdir().unwrap();
        let xml = r#"<mame><machine><rom name="x" crc="AAAAAAAA"/></machine></mame>"#;
        let path = write(dir.path(), "unnamed.xml", xml);
        let outcome = parse_mame_listxml(&path, DatLimits::default()).unwrap();
        assert!(outcome.dat.games.is_empty());
        assert!(
            outcome
                .dat
                .source
                .parse_warnings
                .iter()
                .any(|warning| warning.contains("machine without name skipped"))
        );
    }

    #[test]
    fn malformed_xml_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "bad.xml", "<mame><machine");
        assert!(parse_mame_listxml(&path, DatLimits::default()).is_err());
    }

    #[test]
    fn file_over_size_limit_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "big.xml", "<mame></mame>");
        let mut limits = DatLimits::default();
        limits.max_file_size = 4;
        assert!(matches!(
            parse_mame_listxml(&path, limits),
            Err(ParseError::FileTooLarge { .. })
        ));
    }
}
