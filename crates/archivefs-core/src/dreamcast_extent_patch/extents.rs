//! Strict write eligibility layered on the existing read-only ISO9660 parser.
use super::*;
use crate::iso9660::{self, Iso9660Entry};

const MAX_FILES: usize = 16_384;
const MAX_DIRECTORY_BYTES: u64 = 32 * 1024 * 1024;

/// Coordinates are explicit: never guess a GD-ROM LBA bias from filenames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ExtentCoordinates {
    DiscLba,
    TrackRelativeLba,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct FileExtent {
    pub path: String,
    pub logical_extent_lba: u32,
    pub disc_lba: u64,
    pub byte_length: u64,
    pub sector_count: u64,
    pub source_track: u32,
    pub sector_format: SectorFormat,
    pub track_relative_sector: u64,
    pub track_byte_offset: u64,
    pub source_sha256: String,
}
#[derive(Clone, Debug)]
pub(super) struct ExtentMap(pub BTreeMap<String, FileExtent>);

fn both32(b: &[u8]) -> io::Result<u32> {
    let a = u32::from_le_bytes(b[..4].try_into().unwrap());
    if a != u32::from_be_bytes(b[4..8].try_into().unwrap()) {
        return Err(refuse("inconsistent ISO both-endian value"));
    }
    Ok(a)
}
fn both16(b: &[u8]) -> io::Result<u16> {
    let a = u16::from_le_bytes(b[..2].try_into().unwrap());
    if a != u16::from_be_bytes(b[2..4].try_into().unwrap()) {
        return Err(refuse("inconsistent ISO both-endian value"));
    }
    Ok(a)
}
fn relative(lba: u32, track: &GdiTrack, coordinates: ExtentCoordinates) -> io::Result<u64> {
    match coordinates {
        ExtentCoordinates::DiscLba => u64::from(lba)
            .checked_sub(u64::from(track.start_lba))
            .ok_or_else(|| refuse("extent precedes selected data track")),
        ExtentCoordinates::TrackRelativeLba => Ok(u64::from(lba)),
    }
}
fn range(
    lba: u32,
    size: u64,
    track: &GdiTrack,
    coordinates: ExtentCoordinates,
) -> io::Result<(u64, u64)> {
    let start = relative(lba, track, coordinates)?;
    let end = start
        .checked_add(size.div_ceil(2048))
        .ok_or_else(|| refuse("extent overflow"))?;
    let frames = track.source_length_bytes / u64::from(track.sector_size.bytes());
    if end > frames {
        return Err(refuse("extent exceeds source track"));
    }
    Ok((start, end))
}
fn add_range(ranges: &mut Vec<(u64, u64)>, new: (u64, u64)) -> io::Result<()> {
    if new.0 != new.1 && ranges.iter().any(|r| new.0 < r.1 && r.0 < new.1) {
        return Err(refuse("overlapping filesystem extents/metadata refused"));
    }
    ranges.push(new);
    Ok(())
}
/// The generic reader intentionally observes more than we permit to write.
/// Check raw records too: extended attributes, interleave, multi-extent,
/// associated records, foreign volume references and non-ASCII identifiers.
fn strict_directory(media: &TrackMedia, start: u64, size: u32) -> io::Result<()> {
    let mut consumed = 0u64;
    while consumed < u64::from(size) {
        let mut b = [0; 2048];
        let take = (u64::from(size) - consumed).min(2048) as usize;
        media
            .read_at(start * 2048 + consumed, &mut b[..take])
            .map_err(refuse)?;
        let mut cursor = 0;
        while cursor < take && b[cursor] != 0 {
            let n = b[cursor] as usize;
            if n < 34 || cursor + n > take {
                return Err(refuse("invalid ISO record boundary"));
            }
            let r = &b[cursor..cursor + n];
            let len = r[32] as usize;
            if len == 0
                || 33 + len > n
                || n != ((33 + len + 1) & !1)
                || r[1] != 0
                || r[25] & !3 != 0
                || r[26] != 0
                || r[27] != 0
                || both16(&r[28..32])? != 1
            {
                return Err(refuse(
                    "unsupported ISO record attributes/interleave/multi-extent/volume/system-use",
                ));
            }
            let name = &r[33..33 + len];
            if name != [0]
                && name != [1]
                && (!name.is_ascii() || name.iter().any(|b| *b < 0x20 || *b == 0x7f))
            {
                return Err(refuse("unsupported ISO identifier encoding"));
            }
            cursor += n;
        }
        consumed += take as u64;
    }
    Ok(())
}
fn safe_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && name.is_ascii()
        && !name.chars().any(char::is_control)
}
struct Walker<'a> {
    media: &'a TrackMedia,
    track: &'a GdiTrack,
    coordinates: ExtentCoordinates,
    format: SectorFormat,
    ranges: Vec<(u64, u64)>,
    files: BTreeMap<String, FileExtent>,
    entries: usize,
    directory_bytes: u64,
}
impl Walker<'_> {
    fn directory(&mut self, lba: u32, size: u32, path: &str, depth: usize) -> io::Result<()> {
        if depth > iso9660::MAX_PATH_DEPTH || size == 0 {
            return Err(refuse("directory depth/length refused"));
        }
        self.directory_bytes += u64::from(size);
        if self.directory_bytes > MAX_DIRECTORY_BYTES {
            return Err(refuse("directory byte bound exceeded"));
        }
        let r = range(lba, u64::from(size), self.track, self.coordinates)?;
        add_range(&mut self.ranges, r)?;
        strict_directory(self.media, r.0, size)?;
        let entries = iso9660::read_directory_entries(
            self.media,
            r.0.try_into().map_err(refuse)?,
            size,
            2048,
        )
        .map_err(refuse)?;
        let mut names = BTreeSet::new();
        for entry in entries {
            self.entries += 1;
            if self.entries > MAX_FILES
                || !safe_component(&entry.comparison_name)
                || !safe_component(&entry.original_name)
                || !names.insert(entry.comparison_name.clone())
            {
                return Err(refuse("unsafe/ambiguous ISO path or entry bound exceeded"));
            }
            let child = if path.is_empty() {
                entry.comparison_name.clone()
            } else {
                format!("{path}/{}", entry.comparison_name)
            };
            if entry.is_directory {
                self.directory(entry.extent_lba, entry.size, &child, depth + 1)?;
            } else {
                self.file(entry, child)?;
            }
        }
        Ok(())
    }
    fn file(&mut self, entry: Iso9660Entry, path: String) -> io::Result<()> {
        let r = range(
            entry.extent_lba,
            u64::from(entry.size),
            self.track,
            self.coordinates,
        )?;
        add_range(&mut self.ranges, r)?;
        let extent = FileExtent {
            path: path.clone(),
            logical_extent_lba: entry.extent_lba,
            disc_lba: u64::from(self.track.start_lba) + r.0,
            byte_length: u64::from(entry.size),
            sector_count: r.1 - r.0,
            source_track: self.track.number,
            sector_format: self.format,
            track_relative_sector: r.0,
            track_byte_offset: r.0 * self.format.bytes(),
            source_sha256: hash_logical(self.media, r.0 * 2048, u64::from(entry.size))?,
        };
        self.files.insert(path, extent);
        Ok(())
    }
}
pub(super) fn map_extents(
    media: &TrackMedia,
    track: &GdiTrack,
    coordinates: ExtentCoordinates,
    format: SectorFormat,
) -> io::Result<ExtentMap> {
    let mut pvd = [0; 2048];
    media.read_at(16 * 2048, &mut pvd).map_err(refuse)?;
    if pvd[0] != 1
        || &pvd[1..6] != b"CD001"
        || pvd[6] != 1
        || both16(&pvd[128..132])? != 2048
        || both16(&pvd[120..124])? != 1
        || both16(&pvd[124..128])? != 1
    {
        return Err(refuse("V1 requires single-volume ISO9660 PVD at sector 16"));
    }
    let volume = u64::from(both32(&pvd[80..88])?);
    let frames = track.source_length_bytes / format.bytes();
    let limit = match coordinates {
        ExtentCoordinates::DiscLba => u64::from(track.start_lba) + frames,
        ExtentCoordinates::TrackRelativeLba => frames,
    };
    if volume != limit {
        return Err(refuse(
            "ISO volume length disagrees with explicit extent coordinates/track",
        ));
    }
    // Refuse supplementary/boot descriptor semantics rather than rewrite an
    // incompletely mapped alternate tree. System area and descriptor protected.
    let mut terminator = [0; 2048];
    media.read_at(17 * 2048, &mut terminator).map_err(refuse)?;
    if terminator[0] != 255 || &terminator[1..6] != b"CD001" || terminator[6] != 1 {
        return Err(refuse("unsupported ISO descriptor sequence"));
    }
    let mut walker = Walker {
        media,
        track,
        coordinates,
        format,
        ranges: vec![(0, 18)],
        files: BTreeMap::new(),
        entries: 0,
        directory_bytes: 0,
    };
    let table_size = u64::from(both32(&pvd[132..140])?);
    if table_size == 0 {
        return Err(refuse("ISO path tables missing"));
    }
    for (offset, be, required) in [
        (140, false, true),
        (144, false, false),
        (148, true, true),
        (152, true, false),
    ] {
        let bytes = pvd[offset..offset + 4].try_into().unwrap();
        let lba = if be {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        };
        if lba == 0 {
            if required {
                return Err(refuse("required path table absent"));
            } else {
                continue;
            }
        }
        add_range(
            &mut walker.ranges,
            range(lba, table_size, track, coordinates)?,
        )?;
    }
    let root = &pvd[156..190];
    if root[0] != 34
        || root[1] != 0
        || root[25] != 2
        || root[26] != 0
        || root[27] != 0
        || both16(&root[28..32])? != 1
        || root[32] != 1
        || root[33] != 0
    {
        return Err(refuse("unsupported root record"));
    }
    walker.directory(both32(&root[2..10])?, both32(&root[10..18])?, "", 0)?;
    Ok(ExtentMap(walker.files))
}
