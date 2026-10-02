//! Bounded, read-only structural inspection of a PDF.
//!
//! This is *not* a renderer and not a general PDF library. It follows the
//! cross-reference data (classic tables, xref streams, hybrid files, `/Prev`
//! chains and object streams) just far enough to learn the document's declared
//! page count, a few metadata strings, whether it is encrypted, and whether it
//! carries features a full reader would treat as active. It never runs any of
//! them: no script, embedded file, form, action or link is executed, decoded or
//! opened. Every read, object, nesting level and stream is bounded by
//! [`ManualLimits`].
//!
//! Known gap: a file whose cross-reference data is damaged is refused as
//! malformed rather than reconstructed by scanning.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use super::detect::ManualFormatEvidence;
use super::{
    ManualActiveContent, ManualCapabilityGap, ManualDocumentId, ManualDocumentKind,
    ManualInspection, ManualLimits, ManualMetadata, ManualPdfObjectRef, ManualReadiness,
    ManualViewerError, ManualWarning,
};

const PDF_HEADER_SCAN: usize = 1024;
const TAIL_SCAN: u64 = 4096;
const FIRST_WINDOW: u64 = 256 * 1024;
const MAX_ARRAY_ITEMS: usize = 4096;
const MAX_DICT_ITEMS: usize = 1024;
const MAX_STRING_BYTES: usize = 4096;
const MAX_NAME_BYTES: usize = 256;
const TOKEN_BUDGET: usize = 400_000;
const MAX_OBJSTM_OBJECTS: i64 = 65_535;
const MAX_OBJSTM_CACHE: usize = 8;
const MAX_RESOLVE_DEPTH: usize = 8;

type Dict = BTreeMap<String, Obj>;

#[derive(Clone, Debug)]
enum Obj {
    Null,
    Bool,
    Int(i64),
    Real,
    Name(String),
    Str(Vec<u8>),
    Array(Vec<Obj>),
    Dict(Dict),
    Ref(ManualPdfObjectRef),
}

impl Obj {
    fn as_dict(&self) -> Option<&Dict> {
        match self {
            Self::Dict(d) => Some(d),
            _ => None,
        }
    }
    fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }
}

enum PErr {
    /// Ran out of data; a larger window may succeed.
    Eof,
    Bad(&'static str),
}

fn malformed(detail: &str) -> ManualViewerError {
    ManualViewerError::Malformed(detail.to_string())
}

// ---------------------------------------------------------------- lexer ----

struct Lexer<'a> {
    buf: &'a [u8],
    pos: usize,
    budget: usize,
    max_depth: usize,
}

fn is_ws(b: u8) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}
fn is_delim(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}
fn is_regular(b: u8) -> bool {
    !is_ws(b) && !is_delim(b)
}

impl<'a> Lexer<'a> {
    fn new(buf: &'a [u8], pos: usize, max_depth: usize) -> Self {
        Self {
            buf,
            pos,
            budget: TOKEN_BUDGET,
            max_depth,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if is_ws(b) {
                self.pos += 1;
            } else if b == b'%' {
                while let Some(c) = self.peek() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> &'a [u8] {
        let start = self.pos;
        while self.peek().is_some_and(is_regular) {
            self.pos += 1;
        }
        &self.buf[start..self.pos]
    }

    fn spend(&mut self) -> Result<(), PErr> {
        self.budget = self.budget.checked_sub(1).ok_or(PErr::Bad("too complex"))?;
        Ok(())
    }

    fn keyword(&mut self, expected: &[u8]) -> Result<(), PErr> {
        self.skip_ws();
        if self.pos >= self.buf.len() {
            return Err(PErr::Eof);
        }
        if self.word() == expected {
            Ok(())
        } else {
            Err(PErr::Bad("unexpected keyword"))
        }
    }

    fn unsigned(&mut self) -> Result<u64, PErr> {
        self.skip_ws();
        if self.pos >= self.buf.len() {
            return Err(PErr::Eof);
        }
        let word = self.word();
        if word.is_empty() || word.len() > 19 || !word.iter().all(u8::is_ascii_digit) {
            return Err(PErr::Bad("expected an unsigned integer"));
        }
        std::str::from_utf8(word)
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or(PErr::Bad("integer out of range"))
    }

    fn object(&mut self, depth: usize) -> Result<Obj, PErr> {
        if depth > self.max_depth {
            return Err(PErr::Bad("nesting too deep"));
        }
        self.spend()?;
        self.skip_ws();
        let Some(b) = self.peek() else {
            return Err(PErr::Eof);
        };
        match b {
            b'/' => {
                self.pos += 1;
                Ok(Obj::Name(self.name()))
            }
            b'(' => self.literal_string(),
            b'<' => {
                if self.buf.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    self.dict(depth)
                } else {
                    self.hex_string()
                }
            }
            b'[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        None => return Err(PErr::Eof),
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Obj::Array(items));
                        }
                        _ => {
                            if items.len() >= MAX_ARRAY_ITEMS {
                                return Err(PErr::Bad("array too large"));
                            }
                            items.push(self.object(depth + 1)?);
                        }
                    }
                }
            }
            b')' | b']' | b'>' | b'{' | b'}' => Err(PErr::Bad("unexpected delimiter")),
            _ => self.scalar(),
        }
    }

    fn name(&mut self) -> String {
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            self.pos += 1;
            if b == b'#' && self.pos + 1 < self.buf.len() {
                let hex = std::str::from_utf8(&self.buf[self.pos..self.pos + 2]).unwrap_or("");
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    self.pos += 2;
                    if out.len() < MAX_NAME_BYTES {
                        out.push(value);
                    }
                    continue;
                }
            }
            if out.len() < MAX_NAME_BYTES {
                out.push(b);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    fn literal_string(&mut self) -> Result<Obj, PErr> {
        self.pos += 1; // (
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    push_capped(&mut out, b);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Obj::Str(out));
                    }
                    push_capped(&mut out, b);
                }
                b'\\' => {
                    let Some(next) = self.peek() else {
                        return Err(PErr::Eof);
                    };
                    self.pos += 1;
                    match next {
                        b'n' => push_capped(&mut out, b'\n'),
                        b'r' => push_capped(&mut out, b'\r'),
                        b't' => push_capped(&mut out, b'\t'),
                        b'b' => push_capped(&mut out, 8),
                        b'f' => push_capped(&mut out, 12),
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut value = u32::from(next - b'0');
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        value = value * 8 + u32::from(d - b'0');
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            push_capped(&mut out, (value & 0xFF) as u8);
                        }
                        other => push_capped(&mut out, other),
                    }
                }
                other => push_capped(&mut out, other),
            }
        }
        Err(PErr::Eof)
    }

    fn hex_string(&mut self) -> Result<Obj, PErr> {
        self.pos += 1; // <
        let mut digits = Vec::new();
        while let Some(b) = self.peek() {
            self.pos += 1;
            match b {
                b'>' => {
                    if digits.len() % 2 == 1 {
                        digits.push(0);
                    }
                    let bytes = digits
                        .chunks(2)
                        .take(MAX_STRING_BYTES)
                        .map(|pair| pair[0] * 16 + pair[1])
                        .collect();
                    return Ok(Obj::Str(bytes));
                }
                b if is_ws(b) => {}
                b => match (b as char).to_digit(16) {
                    Some(d) => {
                        if digits.len() < MAX_STRING_BYTES * 2 {
                            digits.push(d as u8);
                        }
                    }
                    None => return Err(PErr::Bad("bad hex string")),
                },
            }
        }
        Err(PErr::Eof)
    }

    fn dict(&mut self, depth: usize) -> Result<Obj, PErr> {
        let mut map = Dict::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(PErr::Eof),
                Some(b'>') => {
                    if self.buf.get(self.pos + 1) == Some(&b'>') {
                        self.pos += 2;
                        return Ok(Obj::Dict(map));
                    }
                    if self.pos + 1 >= self.buf.len() {
                        return Err(PErr::Eof);
                    }
                    return Err(PErr::Bad("bad dictionary end"));
                }
                Some(b'/') => {
                    self.pos += 1;
                    let key = self.name();
                    let value = self.object(depth + 1)?;
                    if map.len() >= MAX_DICT_ITEMS {
                        return Err(PErr::Bad("dictionary too large"));
                    }
                    map.entry(key).or_insert(value);
                }
                _ => return Err(PErr::Bad("expected a dictionary key")),
            }
        }
    }

    fn scalar(&mut self) -> Result<Obj, PErr> {
        let word = self.word();
        if word.is_empty() {
            return Err(PErr::Bad("unexpected byte"));
        }
        match word {
            b"null" => return Ok(Obj::Null),
            b"true" | b"false" => return Ok(Obj::Bool),
            _ => {}
        }
        let text = std::str::from_utf8(word).map_err(|_| PErr::Bad("bad token"))?;
        if let Ok(int) = text.parse::<i64>() {
            // `n g R` indirect reference?
            if (0..=i64::from(u32::MAX)).contains(&int) {
                let save = self.pos;
                self.skip_ws();
                let gen_word = self.word();
                if !gen_word.is_empty()
                    && gen_word.len() <= 5
                    && gen_word.iter().all(u8::is_ascii_digit)
                {
                    self.skip_ws();
                    let r = self.word();
                    if r == b"R" {
                        let generation = std::str::from_utf8(gen_word)
                            .ok()
                            .and_then(|s| s.parse::<u16>().ok())
                            .ok_or(PErr::Bad("bad reference generation"))?;
                        return Ok(Obj::Ref(ManualPdfObjectRef {
                            object_number: int as u32,
                            generation,
                        }));
                    }
                }
                self.pos = save;
            }
            return Ok(Obj::Int(int));
        }
        if text.parse::<f64>().is_ok() {
            return Ok(Obj::Real);
        }
        Err(PErr::Bad("unrecognised token"))
    }
}

fn push_capped(out: &mut Vec<u8>, b: u8) {
    if out.len() < MAX_STRING_BYTES {
        out.push(b);
    }
}

// ----------------------------------------------------- objects & streams ----

#[derive(Clone, Copy, Debug)]
enum XEntry {
    Offset { abs: u64, generation: u16 },
    InStream { stm: u32, index: u32 },
    Free,
}

struct StreamLoc {
    /// Absolute file offset of the first data byte.
    data_offset: u64,
    length: Option<i64>,
    length_ref: Option<ManualPdfObjectRef>,
}

struct ObjStm {
    data: Vec<u8>,
    first: usize,
    entries: Vec<(u32, usize)>,
}

struct Reader<'f> {
    file: &'f mut File,
    file_len: u64,
    header_offset: u64,
    limits: &'f ManualLimits,
    xref: HashMap<u32, XEntry>,
    objstm: HashMap<u32, ObjStm>,
    depth: usize,
}

impl<'f> Reader<'f> {
    fn read_at(&mut self, offset: u64, len: u64) -> Result<Vec<u8>, ManualViewerError> {
        if offset >= self.file_len {
            return Err(malformed("offset beyond end of file"));
        }
        let len = len.min(self.file_len - offset);
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|e| ManualViewerError::Io(e.to_string()))?;
        let mut out = Vec::new();
        (&mut *self.file)
            .take(len)
            .read_to_end(&mut out)
            .map_err(|e| ManualViewerError::Io(e.to_string()))?;
        Ok(out)
    }

    /// Parse the `n g obj <value>` at an absolute offset. A larger window is
    /// tried once if the first one ends mid-object.
    fn load_at(
        &mut self,
        abs: u64,
        expect: Option<ManualPdfObjectRef>,
    ) -> Result<(Obj, Option<StreamLoc>), ManualViewerError> {
        for window in [FIRST_WINDOW, self.limits.pdf_max_object_window] {
            let buf = self.read_at(abs, window)?;
            let complete = (buf.len() as u64) < window || abs + window >= self.file_len;
            match parse_indirect(&buf, abs, self.limits.pdf_max_nesting, expect) {
                Ok(done) => return Ok(done),
                Err(PErr::Eof) if !complete && window == FIRST_WINDOW => continue,
                Err(PErr::Eof) => return Err(malformed("object is truncated")),
                Err(PErr::Bad(why)) => return Err(malformed(why)),
            }
        }
        Err(malformed("object is too large"))
    }

    fn resolve(&mut self, reference: ManualPdfObjectRef) -> Result<Obj, ManualViewerError> {
        if self.depth >= MAX_RESOLVE_DEPTH {
            return Err(malformed("reference chain too deep"));
        }
        self.depth += 1;
        let result = self.resolve_inner(reference);
        self.depth -= 1;
        result
    }

    fn resolve_inner(&mut self, reference: ManualPdfObjectRef) -> Result<Obj, ManualViewerError> {
        match self.xref.get(&reference.object_number).copied() {
            Some(XEntry::Offset { abs, generation }) if generation == reference.generation => {
                Ok(self.load_at(abs, Some(reference))?.0)
            }
            Some(XEntry::InStream { stm, index }) if reference.generation == 0 => {
                self.load_from_object_stream(stm, index, reference.object_number)
            }
            // Undefined references, including stale generations, denote null.
            _ => Ok(Obj::Null),
        }
    }

    /// Follow a reference (one level) and return the value.
    fn deref(&mut self, obj: &Obj) -> Result<Obj, ManualViewerError> {
        match obj {
            Obj::Ref(n) => self.resolve(*n),
            other => Ok(other.clone()),
        }
    }

    fn stream_length(&mut self, loc: &StreamLoc) -> Result<u64, ManualViewerError> {
        let length = match (loc.length, loc.length_ref) {
            (Some(n), _) => n,
            (None, Some(r)) => self
                .resolve(r)?
                .as_int()
                .ok_or_else(|| malformed("bad /Length"))?,
            _ => return Err(malformed("stream has no /Length")),
        };
        u64::try_from(length).map_err(|_| malformed("negative stream length"))
    }

    /// Read and decode a stream's data (Flate only), bounded.
    fn stream_data(&mut self, dict: &Dict, loc: &StreamLoc) -> Result<Vec<u8>, ManualViewerError> {
        let length = self.stream_length(loc)?;
        if length > self.limits.pdf_max_stream_bytes {
            return Err(malformed("stream exceeds the safe size"));
        }
        let raw = self.read_at(loc.data_offset, length)?;
        if (raw.len() as u64) < length {
            return Err(malformed("stream is truncated"));
        }
        let filters = filter_names(dict, self)?;
        let params = decode_params(dict, self)?;
        let mut data = raw;
        for (position, filter) in filters.iter().enumerate() {
            match filter.as_str() {
                "FlateDecode" | "Fl" => {
                    let cap = self.limits.pdf_max_stream_bytes;
                    let mut out = Vec::new();
                    flate2::read::ZlibDecoder::new(&data[..])
                        .take(cap + 1)
                        .read_to_end(&mut out)
                        .map_err(|_| malformed("stream could not be decompressed"))?;
                    if out.len() as u64 > cap {
                        return Err(malformed("stream expands beyond the safe size"));
                    }
                    data = match params.get(position).and_then(|p| p.as_ref()) {
                        Some(p) if p.predictor >= 10 => png_unpredict(&out, p)?,
                        Some(p) if p.predictor == 2 => {
                            return Err(malformed("unsupported TIFF predictor"));
                        }
                        _ => out,
                    };
                }
                _ => return Err(malformed("unsupported stream filter")),
            }
        }
        Ok(data)
    }

    fn load_from_object_stream(
        &mut self,
        stm: u32,
        index: u32,
        num: u32,
    ) -> Result<Obj, ManualViewerError> {
        if !self.objstm.contains_key(&stm) {
            let Some(XEntry::Offset { abs, generation }) = self.xref.get(&stm).copied() else {
                return Err(malformed("object stream is not directly addressable"));
            };
            let (obj, loc) = self.load_at(
                abs,
                Some(ManualPdfObjectRef {
                    object_number: stm,
                    generation,
                }),
            )?;
            let dict = obj
                .as_dict()
                .ok_or_else(|| malformed("bad object stream"))?
                .clone();
            let loc = loc.ok_or_else(|| malformed("object stream has no data"))?;
            let count =
                dict.get("N")
                    .and_then(Obj::as_int)
                    .filter(|n| (0..=MAX_OBJSTM_OBJECTS).contains(n))
                    .ok_or_else(|| malformed("bad object stream count"))? as usize;
            let first = dict
                .get("First")
                .and_then(Obj::as_int)
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| malformed("bad object stream offset"))?;
            let data = self.stream_data(&dict, &loc)?;
            if first > data.len() {
                return Err(malformed("object stream header is truncated"));
            }
            let mut lexer = Lexer::new(&data[..first], 0, self.limits.pdf_max_nesting);
            let mut entries = Vec::with_capacity(count.min(4096));
            for _ in 0..count {
                let number = lexer
                    .unsigned()
                    .map_err(|_| malformed("bad object stream header"))?;
                let offset = lexer
                    .unsigned()
                    .map_err(|_| malformed("bad object stream header"))?;
                entries.push((
                    u32::try_from(number).map_err(|_| malformed("bad object number"))?,
                    usize::try_from(offset).map_err(|_| malformed("bad object offset"))?,
                ));
            }
            if self.objstm.len() >= MAX_OBJSTM_CACHE {
                self.objstm.clear();
            }
            self.objstm.insert(
                stm,
                ObjStm {
                    data,
                    first,
                    entries,
                },
            );
        }
        let cached = self
            .objstm
            .get(&stm)
            .ok_or_else(|| malformed("object stream vanished"))?;
        let slot = cached
            .entries
            .get(index as usize)
            .filter(|(n, _)| *n == num)
            .or_else(|| cached.entries.iter().find(|(n, _)| *n == num))
            .copied()
            .ok_or_else(|| malformed("object not found in its object stream"))?;
        let start = cached
            .first
            .checked_add(slot.1)
            .filter(|s| *s < cached.data.len())
            .ok_or_else(|| malformed("object offset outside its object stream"))?;
        let mut lexer = Lexer::new(&cached.data, start, self.limits.pdf_max_nesting);
        lexer
            .object(0)
            .map_err(|_| malformed("object inside an object stream is malformed"))
    }
}

fn parse_indirect(
    buf: &[u8],
    abs: u64,
    max_depth: usize,
    expect: Option<ManualPdfObjectRef>,
) -> Result<(Obj, Option<StreamLoc>), PErr> {
    let mut lexer = Lexer::new(buf, 0, max_depth);
    let number = lexer.unsigned()?;
    let generation = lexer.unsigned()?;
    lexer.keyword(b"obj")?;
    if let Some(expected) = expect
        && (u64::from(expected.object_number) != number
            || u64::from(expected.generation) != generation)
    {
        return Err(PErr::Bad(
            "object number or generation does not match the cross-reference",
        ));
    }
    let obj = lexer.object(0)?;
    let mut stream = None;
    if let Obj::Dict(dict) = &obj {
        lexer.skip_ws();
        if lexer.word() == b"stream" {
            let mut pos = lexer.pos;
            if buf.get(pos) == Some(&b'\r') && buf.get(pos + 1) == Some(&b'\n') {
                pos += 2;
            } else if matches!(buf.get(pos), Some(b'\n' | b'\r')) {
                pos += 1;
            }
            let (length, length_ref) = match dict.get("Length") {
                Some(Obj::Int(n)) => (Some(*n), None),
                Some(Obj::Ref(r)) => (None, Some(*r)),
                _ => (None, None),
            };
            stream = Some(StreamLoc {
                data_offset: abs + pos as u64,
                length,
                length_ref,
            });
        }
    }
    Ok((obj, stream))
}

// -------------------------------------------------------------- filters ----

struct Params {
    predictor: i64,
    columns: usize,
    bytes_per_pixel: usize,
}

fn filter_names(dict: &Dict, reader: &mut Reader<'_>) -> Result<Vec<String>, ManualViewerError> {
    let Some(filter) = dict.get("Filter") else {
        return Ok(Vec::new());
    };
    let filter = reader.deref(filter)?;
    Ok(match filter {
        Obj::Name(n) => vec![n],
        Obj::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Obj::Name(n) => Ok(n),
                _ => Err(malformed("bad /Filter entry")),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(malformed("bad /Filter")),
    })
}

fn decode_params(
    dict: &Dict,
    reader: &mut Reader<'_>,
) -> Result<Vec<Option<Params>>, ManualViewerError> {
    let Some(raw) = dict.get("DecodeParms").or_else(|| dict.get("DP")) else {
        return Ok(Vec::new());
    };
    let raw = reader.deref(raw)?;
    let items = match raw {
        Obj::Array(items) => items,
        other => vec![other],
    };
    items
        .into_iter()
        .map(|item| match item {
            Obj::Dict(d) => {
                let int =
                    |key: &str, default: i64| d.get(key).and_then(Obj::as_int).unwrap_or(default);
                let columns = usize::try_from(int("Columns", 1))
                    .ok()
                    .filter(|c| (1..=1 << 20).contains(c))
                    .ok_or_else(|| malformed("bad /Columns"))?;
                let colors = int("Colors", 1).clamp(1, 32) as usize;
                let bits = int("BitsPerComponent", 8).clamp(1, 16) as usize;
                Ok(Some(Params {
                    predictor: int("Predictor", 1),
                    columns,
                    bytes_per_pixel: (colors * bits).div_ceil(8).max(1),
                }))
            }
            _ => Ok(None),
        })
        .collect()
}

/// Undo the PNG row predictors used by xref/object streams.
fn png_unpredict(data: &[u8], p: &Params) -> Result<Vec<u8>, ManualViewerError> {
    let row = p
        .columns
        .checked_mul(p.bytes_per_pixel)
        .ok_or_else(|| malformed("bad predictor geometry"))?;
    let stride = row + 1;
    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    let mut previous = vec![0u8; row];
    for chunk in data.chunks(stride) {
        if chunk.len() < stride {
            break;
        }
        let kind = chunk[0];
        let mut current = chunk[1..].to_vec();
        for i in 0..row {
            let left = if i >= p.bytes_per_pixel {
                current[i - p.bytes_per_pixel]
            } else {
                0
            };
            let up = previous[i];
            let up_left = if i >= p.bytes_per_pixel {
                previous[i - p.bytes_per_pixel]
            } else {
                0
            };
            let add = match kind {
                0 => 0,
                1 => left,
                2 => up,
                3 => ((u16::from(left) + u16::from(up)) / 2) as u8,
                4 => paeth(left, up, up_left),
                _ => return Err(malformed("bad predictor row")),
            };
            current[i] = current[i].wrapping_add(add);
        }
        out.extend_from_slice(&current);
        previous = current;
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// ----------------------------------------------------------------- xref ----

enum TableErr {
    /// The window ended mid-table; a larger read may succeed.
    NeedMore,
    Bad(ManualViewerError),
}

struct Section {
    trailer: Dict,
    xref_stream: Option<u64>,
}

impl Reader<'_> {
    fn structural_offset(&self, dict: &Dict, key: &str) -> Result<Option<u64>, ManualViewerError> {
        let Some(value) = dict.get(key) else {
            return Ok(None);
        };
        let offset = value
            .as_int()
            .and_then(|n| u64::try_from(n).ok())
            .and_then(|n| self.header_offset.checked_add(n))
            .filter(|n| *n < self.file_len)
            .ok_or_else(|| malformed("malformed structural cross-reference offset"))?;
        Ok(Some(offset))
    }

    fn merge(&mut self, number: u32, entry: XEntry) -> Result<(), ManualViewerError> {
        if !self.xref.contains_key(&number) {
            if self.xref.len() >= self.limits.pdf_max_objects {
                return Err(malformed("too many objects"));
            }
            self.xref.insert(number, entry);
        }
        Ok(())
    }

    fn read_section(&mut self, abs: u64) -> Result<Section, ManualViewerError> {
        // Small first read; grow once only if a table really needs more. A hostile
        // chain of many sections therefore cannot force large reads per section.
        let windows = [
            64 * 1024,
            self.limits.pdf_max_object_window.saturating_mul(2),
        ];
        for (attempt, window) in windows.into_iter().enumerate() {
            let buf = self.read_at(abs, window)?;
            let mut probe = Lexer::new(&buf, 0, self.limits.pdf_max_nesting);
            probe.skip_ws();
            if !probe.buf[probe.pos..].starts_with(b"xref") {
                return self.read_xref_stream(abs).map(|trailer| Section {
                    trailer,
                    xref_stream: None,
                });
            }
            let more_may_exist = (buf.len() as u64) == window && abs + window < self.file_len;
            match self.read_table(&buf, probe.pos + 4) {
                Ok(section) => return Ok(section),
                Err(TableErr::NeedMore) if attempt == 0 && more_may_exist => continue,
                Err(TableErr::NeedMore) => {
                    return Err(malformed("cross-reference table is truncated or too large"));
                }
                Err(TableErr::Bad(error)) => return Err(error),
            }
        }
        Err(malformed("cross-reference table is too large"))
    }

    fn read_table(&mut self, buf: &[u8], start: usize) -> Result<Section, TableErr> {
        let bad = |why: &str| TableErr::Bad(malformed(why));
        let number = |lexer: &mut Lexer<'_>, why: &str| -> Result<u64, TableErr> {
            lexer.unsigned().map_err(|e| match e {
                PErr::Eof => TableErr::NeedMore,
                PErr::Bad(_) => TableErr::Bad(malformed(why)),
            })
        };
        let mut lexer = Lexer::new(buf, start, self.limits.pdf_max_nesting);
        loop {
            lexer.skip_ws();
            if lexer.pos >= buf.len() {
                return Err(TableErr::NeedMore);
            }
            if buf[lexer.pos..].starts_with(b"trailer") {
                lexer.pos += 7;
                break;
            }
            let first = number(&mut lexer, "bad cross-reference table")?;
            let count = number(&mut lexer, "bad cross-reference table")?;
            if count > self.limits.pdf_max_objects as u64 {
                return Err(bad("cross-reference table is too large"));
            }
            for i in 0..count {
                let offset = number(&mut lexer, "bad cross-reference entry")?;
                let generation = u16::try_from(number(&mut lexer, "bad cross-reference entry")?)
                    .map_err(|_| bad("bad cross-reference generation"))?;
                lexer.skip_ws();
                if lexer.pos >= buf.len() {
                    return Err(TableErr::NeedMore);
                }
                let kind = lexer.word();
                let object = u32::try_from(
                    first
                        .checked_add(i)
                        .ok_or_else(|| bad("object number overflow"))?,
                )
                .map_err(|_| bad("object number too large"))?;
                let entry = match kind {
                    b"n" => match self
                        .header_offset
                        .checked_add(offset)
                        .filter(|o| *o < self.file_len)
                    {
                        Some(abs) if offset > 0 => XEntry::Offset { abs, generation },
                        _ => return Err(bad("live cross-reference offset outside file")),
                    },
                    b"f" => XEntry::Free,
                    _ => return Err(bad("bad cross-reference entry type")),
                };
                self.merge(object, entry).map_err(TableErr::Bad)?;
            }
        }
        let trailer = match lexer.object(0) {
            Ok(Obj::Dict(d)) => d,
            Err(PErr::Eof) => return Err(TableErr::NeedMore),
            _ => return Err(bad("bad trailer")),
        };
        let xref_stream = self
            .structural_offset(&trailer, "XRefStm")
            .map_err(TableErr::Bad)?;
        Ok(Section {
            trailer,
            xref_stream,
        })
    }

    fn read_xref_stream(&mut self, abs: u64) -> Result<Dict, ManualViewerError> {
        let (obj, loc) = self.load_at(abs, None)?;
        let dict = obj
            .as_dict()
            .ok_or_else(|| malformed("bad cross-reference stream"))?
            .clone();
        if !matches!(dict.get("Type"), Some(Obj::Name(n)) if n == "XRef") {
            return Err(malformed(
                "cross-reference data not found at the recorded offset",
            ));
        }
        let loc = loc.ok_or_else(|| malformed("cross-reference stream has no data"))?;
        let data = self.stream_data(&dict, &loc)?;
        let widths: Vec<usize> = match dict.get("W") {
            Some(Obj::Array(items)) if items.len() == 3 => items
                .iter()
                .map(|i| {
                    i.as_int()
                        .and_then(|n| usize::try_from(n).ok())
                        .filter(|n| *n <= 8)
                })
                .collect::<Option<_>>()
                .ok_or_else(|| malformed("bad /W"))?,
            _ => return Err(malformed("bad /W")),
        };
        let size = dict
            .get("Size")
            .and_then(Obj::as_int)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| malformed("bad /Size"))?;
        let index: Vec<(u64, u64)> = match dict.get("Index") {
            Some(Obj::Array(items)) if items.len() % 2 == 0 => items
                .chunks(2)
                .map(|pair| {
                    Some((
                        u64::try_from(pair[0].as_int()?).ok()?,
                        u64::try_from(pair[1].as_int()?).ok()?,
                    ))
                })
                .collect::<Option<_>>()
                .ok_or_else(|| malformed("bad /Index"))?,
            None => vec![(0, size)],
            _ => return Err(malformed("bad /Index")),
        };
        let row: usize = widths.iter().sum();
        if row == 0 {
            return Err(malformed("empty cross-reference rows"));
        }
        let field = |bytes: &[u8]| bytes.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b));
        let mut cursor = 0usize;
        for (first, count) in index {
            if count > self.limits.pdf_max_objects as u64 {
                return Err(malformed("cross-reference stream is too large"));
            }
            for i in 0..count {
                let Some(slice) = data.get(cursor..cursor + row) else {
                    return Err(malformed("cross-reference stream is truncated"));
                };
                cursor += row;
                let kind = if widths[0] == 0 {
                    1
                } else {
                    field(&slice[..widths[0]])
                };
                let second = field(&slice[widths[0]..widths[0] + widths[1]]);
                let third = field(&slice[widths[0] + widths[1]..]);
                let number = u32::try_from(
                    first
                        .checked_add(i)
                        .ok_or_else(|| malformed("object number overflow"))?,
                )
                .map_err(|_| malformed("object number too large"))?;
                match kind {
                    0 => self.merge(number, XEntry::Free)?,
                    1 => match self
                        .header_offset
                        .checked_add(second)
                        .filter(|o| *o < self.file_len)
                    {
                        Some(abs) if second > 0 => self.merge(
                            number,
                            XEntry::Offset {
                                abs,
                                generation: u16::try_from(third)
                                    .map_err(|_| malformed("bad cross-reference generation"))?,
                            },
                        )?,
                        _ => return Err(malformed("live cross-reference offset outside file")),
                    },
                    2 => self.merge(
                        number,
                        XEntry::InStream {
                            stm: u32::try_from(second)
                                .map_err(|_| malformed("bad object stream number"))?,
                            index: u32::try_from(third)
                                .map_err(|_| malformed("bad object stream index"))?,
                        },
                    )?,
                    _ => {}
                }
            }
        }
        // Hybrid streams do not drive the update chain, but damaged references
        // inside them must not disappear merely because their trailer is unused.
        self.structural_offset(&dict, "Prev")?;
        Ok(dict)
    }
}

// ------------------------------------------------------------- top level ----

pub(super) fn page_index(
    file: &mut File,
    file_len: u64,
    limits: &ManualLimits,
) -> Result<Vec<ManualPdfObjectRef>, ManualViewerError> {
    let (mut reader, catalog, _) = open_catalog(file, file_len, limits)?;
    if !matches!(catalog.get("Type"), Some(Obj::Name(name)) if name == "Catalog") {
        return Err(malformed("bad catalog type"));
    }
    let Some(Obj::Ref(root)) = catalog.get("Pages") else {
        return Err(malformed("page tree root must be an indirect reference"));
    };
    let mut pages = Vec::new();
    reader.walk_pages(*root, None, 0, &mut HashSet::new(), &mut pages)?;
    Ok(pages)
}

impl Reader<'_> {
    fn walk_pages(
        &mut self,
        reference: ManualPdfObjectRef,
        parent: Option<ManualPdfObjectRef>,
        depth: usize,
        visited: &mut HashSet<ManualPdfObjectRef>,
        pages: &mut Vec<ManualPdfObjectRef>,
    ) -> Result<(), ManualViewerError> {
        if depth >= self.limits.pdf_max_nesting {
            return Err(malformed("page tree exceeds nesting bound"));
        }
        if visited.len() >= self.limits.pdf_max_objects {
            return Err(malformed("page tree exceeds object bound"));
        }
        if !visited.insert(reference) {
            return Err(malformed("page tree contains a cycle or repeated child"));
        }
        let obj = self.resolve(reference)?;
        let node = obj
            .as_dict()
            .ok_or_else(|| malformed("bad page tree node"))?;
        match (parent, node.get("Parent")) {
            (None, None) => {}
            (Some(expected), Some(Obj::Ref(actual))) if expected == *actual => {}
            _ => return Err(malformed("page tree parent does not match its position")),
        }
        match node.get("Type") {
            Some(Obj::Name(name)) if name == "Pages" => {
                let count = self
                    .deref(node.get("Count").unwrap_or(&Obj::Null))?
                    .as_int()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| malformed("bad page tree count"))?;
                if count > self.limits.max_pages {
                    return Err(ManualViewerError::TooManyPages {
                        count,
                        max: self.limits.max_pages,
                    });
                }
                let Obj::Array(kids) = self.deref(node.get("Kids").unwrap_or(&Obj::Null))? else {
                    return Err(malformed("page tree has no child array"));
                };
                if kids.is_empty() {
                    return Err(malformed("page tree has no children"));
                }
                let start = pages.len();
                for child in kids {
                    let Obj::Ref(child) = child else {
                        return Err(malformed("page tree children must be indirect references"));
                    };
                    self.walk_pages(child, Some(reference), depth + 1, visited, pages)?;
                }
                if pages.len() - start != count {
                    return Err(malformed(
                        "page tree count differs from actual descendant pages",
                    ));
                }
            }
            Some(Obj::Name(name)) if name == "Page" && parent.is_some() => {
                if pages.len() >= self.limits.max_pages {
                    return Err(ManualViewerError::TooManyPages {
                        count: pages.len() + 1,
                        max: self.limits.max_pages,
                    });
                }
                pages.push(reference);
            }
            _ => return Err(malformed("bad page tree node type")),
        }
        Ok(())
    }
}

fn open_catalog<'a>(
    file: &'a mut File,
    file_len: u64,
    limits: &'a ManualLimits,
) -> Result<(Reader<'a>, Dict, Option<Obj>), ManualViewerError> {
    let mut head = Vec::new();
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ManualViewerError::Io(e.to_string()))?;
    (&mut *file)
        .take(PDF_HEADER_SCAN as u64)
        .read_to_end(&mut head)
        .map_err(|e| ManualViewerError::Io(e.to_string()))?;
    let header_offset = head
        .windows(5)
        .position(|w| w == b"%PDF-")
        .ok_or(ManualViewerError::UnrecognisedFormat)? as u64;

    let mut reader = Reader {
        file,
        file_len,
        header_offset,
        limits,
        xref: HashMap::new(),
        objstm: HashMap::new(),
        depth: 0,
    };

    // startxref in the tail. Some files are padded with NULs/whitespace to a
    // round size, which can push the real end megabytes from the end of the
    // file, so skip trailing padding (bounded) before looking.
    let mut end = file_len;
    let mut skipped = 0u64;
    loop {
        let from = end.saturating_sub(TAIL_SCAN);
        let chunk = reader.read_at(from, end - from)?;
        let kept = chunk.iter().rposition(|b| !is_ws(*b)).map_or(0, |i| i + 1);
        if kept > 0 {
            end = from + kept as u64;
            break;
        }
        skipped += chunk.len() as u64;
        if from == 0 || skipped > limits.pdf_max_tail_padding {
            return Err(malformed("no startxref (the file ends in padding)"));
        }
        end = from;
    }
    let tail_start = end.saturating_sub(TAIL_SCAN);
    let tail = reader.read_at(tail_start, end - tail_start)?;
    let marker = tail
        .windows(9)
        .rposition(|w| w == b"startxref")
        .ok_or_else(|| malformed("no startxref (truncated or not a complete PDF)"))?;
    let mut lexer = Lexer::new(&tail, marker + 9, limits.pdf_max_nesting);
    let startxref = lexer.unsigned().map_err(|_| malformed("bad startxref"))?;

    // Walk the cross-reference chain, newest section first.
    let mut visited = HashSet::new();
    let mut trailers: Vec<Dict> = Vec::new();
    let mut next = Some(
        header_offset
            .checked_add(startxref)
            .filter(|o| *o < file_len)
            .ok_or_else(|| malformed("startxref points outside the file"))?,
    );
    while let Some(abs) = next.take() {
        if !visited.insert(abs) {
            return Err(malformed("cross-reference chain loops"));
        }
        if visited.len() > limits.pdf_max_xref_sections {
            return Err(malformed("too many cross-reference sections"));
        }
        let section = reader.read_section(abs)?;
        if section.trailer.contains_key("Encrypt") {
            return Err(ManualViewerError::Encrypted);
        }
        if let Some(stream_abs) = section.xref_stream {
            if !visited.insert(stream_abs) || visited.len() > limits.pdf_max_xref_sections {
                return Err(malformed(
                    "cross-reference chain loops or exceeds depth bound",
                ));
            }
            reader.read_xref_stream(stream_abs)?;
        }
        next = reader.structural_offset(&section.trailer, "Prev")?;
        trailers.push(section.trailer);
    }
    let newest = trailers.first().ok_or_else(|| malformed("no trailer"))?;
    let root = match newest.get("Root") {
        Some(Obj::Ref(n)) => *n,
        _ => return Err(malformed("no document catalog")),
    };
    let info = newest.get("Info").cloned();

    let catalog_obj = reader.resolve(root)?;
    let catalog = catalog_obj
        .as_dict()
        .ok_or_else(|| malformed("bad document catalog"))?
        .clone();
    Ok((reader, catalog, info))
}

pub(super) fn inspect(
    file: &mut File,
    id: ManualDocumentId,
    evidence: ManualFormatEvidence,
    limits: &ManualLimits,
) -> Result<ManualInspection, ManualViewerError> {
    let (mut reader, catalog, info) = open_catalog(file, id.len, limits)?;
    let pages_ref = catalog
        .get("Pages")
        .cloned()
        .ok_or_else(|| malformed("catalog has no page tree"))?;
    let pages_obj = reader.deref(&pages_ref)?;
    let pages = pages_obj
        .as_dict()
        .ok_or_else(|| malformed("bad page tree"))?;
    let count_obj = pages
        .get("Count")
        .cloned()
        .ok_or_else(|| malformed("page tree has no /Count"))?;
    let count = reader
        .deref(&count_obj)?
        .as_int()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| malformed("bad page count"))?;
    if count > limits.max_pages {
        return Err(ManualViewerError::TooManyPages {
            count,
            max: limits.max_pages,
        });
    }

    let mut active = Vec::new();
    for (key, flag) in [
        ("OpenAction", ManualActiveContent::OpenAction),
        ("AA", ManualActiveContent::AdditionalActions),
        ("AcroForm", ManualActiveContent::InteractiveForm),
        ("AF", ManualActiveContent::EmbeddedFiles),
    ] {
        if catalog.contains_key(key) {
            active.push(flag);
        }
    }
    if let Some(names) = catalog.get("Names")
        && let Ok(Obj::Dict(names)) = reader.deref(names)
    {
        if names.contains_key("JavaScript") {
            active.push(ManualActiveContent::JavaScript);
        }
        if names.contains_key("EmbeddedFiles") {
            active.push(ManualActiveContent::EmbeddedFiles);
        }
    }
    if let Some(action) = catalog.get("OpenAction")
        && let Obj::Dict(action) = reader.deref(action)?
        && (matches!(action.get("S"), Some(Obj::Name(name)) if name == "JavaScript")
            || action.contains_key("JS"))
    {
        active.push(ManualActiveContent::JavaScript);
    }
    active.sort();
    active.dedup();

    let mut metadata = ManualMetadata::default();
    if let Some(info) = info
        && let Ok(Obj::Dict(info)) = reader.deref(&info)
    {
        let text = |reader: &mut Reader<'_>, key: &str| -> Option<String> {
            let value = reader.deref(info.get(key)?).ok()?;
            match value {
                Obj::Str(bytes) => {
                    Some(decode_text(&bytes, limits.max_metadata_chars)).filter(|s| !s.is_empty())
                }
                _ => None,
            }
        };
        metadata.title = text(&mut reader, "Title");
        metadata.author = text(&mut reader, "Author");
        metadata.producer = text(&mut reader, "Producer");
        metadata.created = text(&mut reader, "CreationDate");
    }

    let mut warnings = vec![ManualWarning::PageCountIsDeclared];
    if !active.is_empty() {
        warnings.push(ManualWarning::ActiveContentIgnored);
    }
    Ok(ManualInspection {
        id,
        kind: ManualDocumentKind::Pdf,
        evidence,
        readiness: ManualReadiness::InspectOnly {
            missing: ManualCapabilityGap::PdfRenderer,
        },
        page_count: Some(count),
        pages: Vec::new(),
        metadata,
        active_content: active,
        warnings,
    })
}

/// PDF text string (UTF-16BE with BOM, else Latin-1-ish PDFDocEncoding),
/// stripped of control characters and cut to `max_chars`.
fn decode_text(bytes: &[u8], max_chars: usize) -> String {
    let text: String = if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|b| char::from(*b)).collect()
    };
    text.chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}
