//! Validate structure and discover byte spans without reserializing user XML.
use super::*;
use quick_xml::events::Event;
use std::collections::BTreeSet;

pub(super) struct GamelistXml {
    pub offset: usize,
    pub empty_root: bool,
    pub paths: BTreeSet<String>,
    /// Direct children of `<gameList>`, all preserved byte-for-byte.
    pub games: usize,
    pub folders: usize,
    pub other_elements: usize,
}

pub(super) fn inspect(text: &str, path: &Path) -> Result<GamelistXml, EsDePublicationError> {
    let fail = |detail: String| EsDePublicationError::MalformedGamelist {
        path: path.into(),
        detail,
    };
    if text.chars().any(|c| !xml_character(c)) {
        return Err(fail("invalid XML character".into()));
    }
    // quick-xml drops a leading UTF-8 BOM without counting it in
    // `buffer_position`, so inspect the text after it and shift every span by
    // its length; otherwise insertion offsets land three bytes early.
    let bom = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let mut reader = quick_xml::Reader::from_str(&text[bom..]);
    reader.config_mut().check_comments = true;
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut declaration_seen = false;
    let mut root_seen = false;
    let mut root_closed = false;
    let mut offset = None;
    let mut empty_root = false;
    let mut paths = BTreeSet::new();
    let mut game_path = String::new();
    let (mut games, mut folders, mut other_elements) = (0, 0, 0);
    loop {
        let before = reader.buffer_position() as usize;
        let event = reader.read_event().map_err(|e| fail(e.to_string()))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                if root_closed || stack.len() >= 256 {
                    return Err(fail("multiple roots or excessive nesting".into()));
                }
                let name = e.name().as_ref().to_vec();
                if stack.is_empty() {
                    if root_seen || name != b"gameList" {
                        return Err(fail("expected one gameList root".into()));
                    }
                    root_seen = true;
                }
                if !xml_name(&name) {
                    return Err(fail("invalid XML element name".into()));
                }
                for attr in e.attributes() {
                    let attr = attr.map_err(|e| fail(e.to_string()))?;
                    if !xml_name(attr.key.as_ref()) {
                        return Err(fail("invalid XML attribute name".into()));
                    }
                    let value = attr
                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                        .map_err(|e| fail(e.to_string()))?;
                    if value.chars().any(|c| !xml_character(c)) {
                        return Err(fail("invalid XML attribute character".into()));
                    }
                }
                if stack.len() == 3 && stack[1] == b"game" && stack[2] == b"path" {
                    return Err(fail("nested markup in game path".into()));
                }
                if stack.len() == 1 {
                    match name.as_slice() {
                        b"game" => games += 1,
                        b"folder" => folders += 1,
                        _ => other_elements += 1,
                    }
                }
                if stack.len() == 2 && stack[1] == b"game" && name == b"path" {
                    game_path.clear();
                }
                if empty {
                    if stack.is_empty() {
                        root_closed = true;
                        empty_root = true;
                        offset = Some(reader.buffer_position() as usize - 2);
                    }
                } else {
                    stack.push(name);
                }
            }
            Event::End(e) => {
                if stack.len() == 3 && stack[1] == b"game" && stack[2] == b"path" {
                    // Not trimmed: whether ES-DE trims is not established, so
                    // padded paths are handled fail-closed by `matching`.
                    paths.insert(game_path.clone());
                }
                if stack.pop().as_deref() != Some(e.name().as_ref()) {
                    return Err(fail("mismatched closing element".into()));
                }
                if stack.is_empty() {
                    root_closed = true;
                    offset = Some(before);
                }
            }
            Event::Text(e) => {
                let decoded = e.decode().map_err(|e| fail(e.to_string()))?;
                if stack.is_empty() && !decoded.trim().is_empty() {
                    return Err(fail("text outside root".into()));
                }
                if decoded
                    .chars()
                    .any(|c| c < ' ' && !matches!(c, '\t' | '\r' | '\n'))
                {
                    return Err(fail("invalid XML character".into()));
                }
                if stack.len() == 3 && stack[1] == b"game" && stack[2] == b"path" {
                    game_path.push_str(&decoded);
                }
            }
            Event::CData(e) => {
                if stack.is_empty() {
                    return Err(fail("CDATA outside root".into()));
                }
                if stack.len() == 3 && stack[1] == b"game" && stack[2] == b"path" {
                    game_path.push_str(&e.decode().map_err(|e| fail(e.to_string()))?);
                }
            }
            Event::GeneralRef(e) => {
                if stack.is_empty() {
                    return Err(fail("entity outside root".into()));
                }
                let value =
                    if let Some(c) = e.resolve_char_ref().map_err(|e| fail(e.to_string()))? {
                        c.to_string()
                    } else {
                        quick_xml::escape::resolve_predefined_entity(
                            &e.decode().map_err(|e| fail(e.to_string()))?,
                        )
                        .ok_or_else(|| fail("unsupported entity reference".into()))?
                        .to_string()
                    };
                if value.chars().any(|c| !xml_character(c)) {
                    return Err(fail("invalid XML entity character".into()));
                }
                if stack.len() == 3 && stack[1] == b"game" && stack[2] == b"path" {
                    game_path.push_str(&value);
                }
            }
            Event::DocType(_) => return Err(fail("DTD is unsupported".into())),
            Event::Decl(e) => {
                if root_seen || declaration_seen || before != 0 {
                    return Err(fail("misplaced XML declaration".into()));
                }
                declaration_seen = true;
                if e.version().map_err(|e| fail(e.to_string()))?.as_ref() != b"1.0" {
                    return Err(fail("only XML 1.0 is supported".into()));
                }
                if let Some(encoding) = e.encoding() {
                    if !encoding
                        .map_err(|e| fail(e.to_string()))?
                        .eq_ignore_ascii_case(b"utf-8")
                    {
                        return Err(fail("only UTF-8 XML is supported".into()));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !root_seen || !root_closed || !stack.is_empty() {
        return Err(fail("incomplete gameList document".into()));
    }
    let offset = bom + offset.ok_or_else(|| fail("missing root closing span".into()))?;
    let span = if empty_root { "/>" } else { "</" };
    if !text.is_char_boundary(offset) || !text[offset..].starts_with(span) {
        return Err(fail(
            "root closing span could not be located exactly".into(),
        ));
    }
    Ok(GamelistXml {
        offset,
        empty_root,
        paths,
        games,
        folders,
        other_elements,
    })
}

fn xml_character(c: char) -> bool {
    matches!(c, '\t' | '\r' | '\n')
        || matches!(c as u32, 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}
fn name_start(c: char) -> bool {
    c == ':'
        || c == '_'
        || c.is_ascii_alphabetic()
        || matches!(c as u32,
        0xc0..=0xd6 | 0xd8..=0xf6 | 0xf8..=0x2ff | 0x370..=0x37d | 0x37f..=0x1fff |
        0x200c..=0x200d | 0x2070..=0x218f | 0x2c00..=0x2fef | 0x3001..=0xd7ff |
        0xf900..=0xfdcf | 0xfdf0..=0xfffd | 0x10000..=0xeffff)
}
fn xml_name(bytes: &[u8]) -> bool {
    let Ok(name) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut chars = name.chars();
    chars.next().is_some_and(name_start)
        && chars.all(|c| {
            name_start(c)
                || c.is_ascii_digit()
                || matches!(c, '-' | '.' | '\u{b7}')
                || matches!(c as u32, 0x300..=0x36f | 0x203f..=0x2040)
        })
}
