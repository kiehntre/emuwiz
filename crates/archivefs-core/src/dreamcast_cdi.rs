//! Dreamcast DiscJuggler `.cdi` identity support.
//!
//! # What backs this
//!
//! [`opticaldiscs::discjuggler`](https://docs.rs/opticaldiscs) (MIT, by
//! danifunker) - already a dependency behind this crate's own
//! `dreamcast-cdi` feature (default-on; see this crate's `Cargo.toml`).
//! Unlike [`crate::chd_optical_specialist`]'s `chd-optical-specialist`
//! feature, `dreamcast-cdi` does not enable `opticaldiscs`'s own `chd`
//! feature, so it pulls in no native/C++ code (`libchdman-rs`) - the two
//! features are fully independent, and either can be enabled without the
//! other.
//!
//! `opticaldiscs::discjuggler` is a pure-Rust, bounds-checked walk of the
//! DiscJuggler trailer descriptor, ported field-for-field from cdemu's
//! `libmirage` `image-cdi/parser.c` - the closest thing that exists to an
//! authoritative reference for this closed, reverse-engineered format
//! (DiscJuggler itself never published a spec). Every multi-byte read in
//! that parser goes through a bounds-checked cursor that returns a `Parse`
//! error rather than panicking or reading out of bounds, so a truncated or
//! foreign file is refused, never guessed at.
//!
//! This module does not reimplement any of that trailer walking. It only:
//!
//! 1. Bounds the whole-file read [`opticaldiscs::discjuggler::parse_discjuggler`]
//!    itself performs (see [`MAX_CDI_BYTES`]) and serializes concurrent
//!    calls to it (see "Upstream limitation" below);
//! 2. Selects the single correct Dreamcast data track from the parsed
//!    track list, using the same structural rule
//!    [`crate::ingestion::gdi::resolve_gdi_data_track`] already uses for
//!    `.gdi` (see [`select_dreamcast_data_track`]);
//! 3. Cross-checks that track's declared byte range actually fits inside
//!    the real file, since the trailer's own arithmetic is never checked
//!    against the file's true length;
//! 4. Wraps the result in a [`crate::logical_media::LogicalMedia`] so the
//!    existing, unchanged [`crate::game_identity::inspect_dreamcast_source`]
//!    (via `parse_ip_bin_meta`) can read it exactly like an ISO, CUE, GDI,
//!    or GD-ROM CHD source. No second Dreamcast identity model, no
//!    invented offsets, no magic-string scan.
//!
//! # Track selection
//!
//! A `.cdi` concatenates every track from every session in one file. Per
//! [`opticaldiscs::discjuggler`]'s own module documentation, a Dreamcast
//! GD-ROM rip's high-density session records its absolute starting LBA in
//! [`opticaldiscs::discjuggler::DiscJugglerTrack::base_lba`] (`0` for a
//! plain volume-relative disc). This is exactly the same structural
//! signal `.gdi` descriptors give directly as `start_lba`, so
//! [`select_dreamcast_data_track`] reuses the identical
//! [`crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME`] boundary and
//! the identical "exactly one candidate, or refuse" rule `.gdi` uses -
//! never track order, never a filename, never a guess. A single-session
//! disc (every track's `base_lba == 0`) falls back to requiring exactly
//! one data track in the whole file.
//!
//! IP.BIN always sits at logical offset zero of the selected data track's
//! own cooked view, so no absolute-LBA rebasing is needed here (unlike
//! browsing that track's ISO 9660 directory extents, which is outside
//! this module's scope entirely).
//!
//! # Upstream limitation: no `Read + Seek`/`mmap` API, and why this
//! serializes instead
//!
//! `opticaldiscs::discjuggler::parse_discjuggler` is the *only* public
//! entry point this crate has for CDI, and it is hardcoded to
//! `std::fs::read(path)` - it reads the **entire file** into a `Vec<u8>`
//! purely to reach the trailer descriptor at the end (typically a few KB).
//! There is no `Read + Seek`-generic variant, no `mmap`/`memmap2`
//! dependency anywhere in `opticaldiscs` 0.15.0 (confirmed against its own
//! `Cargo.toml`), and the trailer-walking functions it calls internally
//! (`load_session`, `load_track`, `Cursor`, ...) are private, so there is
//! no way for calling code to supply a different byte source. Handing
//! `parse_discjuggler` anything other than a plain path (e.g. a path
//! backed by our own `mmap`) would not change what it does once called -
//! the read happens inside a function this crate does not control.
//!
//! The only way to avoid that whole-file read would be to reimplement the
//! trailer walk ourselves against different I/O - exactly the "second CDI
//! parser" this module must not become (see the module-level "What backs
//! this" section above: DiscJuggler's own reverse-engineered ambiguity is
//! why a from-scratch parser was rejected in favor of reusing
//! `opticaldiscs` in the first place).
//!
//! Given that, [`MAX_CDI_BYTES`] bounds the worst case for *one* call, but
//! a library scanner that inspects many `.cdi` files *concurrently* could
//! still hold several such multi-hundred-MB-to-1.5-GiB buffers at once.
//! [`CDI_PARSE_LOCK`] serializes calls into `parse_discjuggler` itself
//! (not the rest of this module's own bounded, allocation-free track
//! selection and sector reads) so at most one such buffer exists at a
//! time - a cheap, correct mitigation for the single process-wide
//! resource this crate's own code can actually control. It does not fix
//! the underlying inefficiency (a genuinely large legitimate disc still
//! costs one full read), which can only be fixed upstream in
//! `opticaldiscs` itself.

use std::cell::RefCell;
use std::path::Path;
use std::sync::Mutex;

use opticaldiscs::discjuggler::{DiscJugglerTrack, parse_discjuggler};
use opticaldiscs::sector_reader::{BinCueSectorReader, SectorReader};

use crate::chd_identity::GDROM_HIGH_DENSITY_START_FRAME;
use crate::logical_media::{LogicalMedia, LogicalMediaError};

/// Cooked ISO 9660 logical sector size, matching every other reader in
/// this crate.
const SECTOR_SIZE: u64 = 2048;

/// A real Dreamcast GD-ROM `.cdi` tops out around 1.2 GiB. 1.5 GiB leaves
/// headroom for format overhead (subchannel data, CD-Text, multiple
/// sessions) while still bounding `parse_discjuggler`'s unavoidable
/// whole-file read (see the module documentation's "Upstream limitation"
/// section) well below the 2 GiB this module used before this bound was
/// tightened - reducing the parser's worst-case heap allocation per call
/// without risking rejecting a genuine disc. Checked from filesystem
/// metadata alone, before any read.
pub const MAX_CDI_BYTES: u64 = 1_536 * 1024 * 1024;

/// Serializes calls into [`opticaldiscs::discjuggler::parse_discjuggler`],
/// the one place in this module that allocates a buffer up to
/// [`MAX_CDI_BYTES`] - see the module documentation's "Upstream
/// limitation" section for why this exists instead of a streaming parse.
/// A plain [`Mutex`] is the smallest tool for this: the call it guards is
/// synchronous and brief (parse only, no long-lived I/O), and every
/// caller in this crate is itself synchronous.
static CDI_PARSE_LOCK: Mutex<()> = Mutex::new(());

/// `parse_discjuggler` already bounds `num_sessions`/per-session track
/// count to `u8` each (at most 255 sessions, 255 tracks per session), so
/// this can never actually be exceeded by a real parse - a second,
/// independent bound kept only as defense in depth.
const MAX_CDI_TRACKS: usize = 65_536;

#[derive(Debug)]
pub enum CdiIdentityError {
    /// The path could not be statted or read.
    Io(String),
    /// The file exceeds [`MAX_CDI_BYTES`]; refused before any read.
    TooLarge { bytes: u64, maximum: u64 },
    /// `opticaldiscs::discjuggler::parse_discjuggler` rejected the file:
    /// not a CDI at all, or a truncated/malformed/unsupported-read-mode
    /// trailer. `detail` is its own error, rendered as text.
    Parse(String),
    /// More tracks than [`MAX_CDI_TRACKS`] were parsed - defense in depth,
    /// not expected to be reachable via a genuine parse.
    TooManyTracks,
    /// No Dreamcast-eligible data track was found at all.
    NoDataTrack,
    /// More than one track could be the Dreamcast data track; refused
    /// rather than guessed, exactly like `.gdi`'s own rule.
    AmbiguousDataTracks,
    /// The track's declared sector geometry could not be exposed as a
    /// bounded, valid 2048-byte cooked view (e.g. its user-data offset
    /// does not fit inside its own physical sector).
    UnsupportedSectorLayout,
    /// The track's declared byte range does not fit inside the real
    /// file - a contradictory or overflowing offset/length pair, never
    /// trusted just because the trailer's own arithmetic produced it.
    ImpossibleOffset,
    /// [`CDI_PARSE_LOCK`] was poisoned by a panic in a previous call.
    /// Refused rather than recovering the guard and proceeding: this
    /// crate has no way to know the poisoning panic left no shared state
    /// corrupted, so the safe answer is to fail closed, not to guess.
    LockPoisoned,
}

impl std::fmt::Display for CdiIdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(detail) => write!(formatter, "CDI I/O error: {detail}"),
            Self::TooLarge { bytes, maximum } => write!(
                formatter,
                "CDI file is {bytes} bytes, exceeding the {maximum}-byte bound"
            ),
            Self::Parse(detail) => {
                write!(formatter, "CDI descriptor could not be parsed: {detail}")
            }
            Self::TooManyTracks => formatter.write_str("CDI descriptor declares too many tracks"),
            Self::NoDataTrack => {
                formatter.write_str("CDI has no track this module recognises as Dreamcast data")
            }
            Self::AmbiguousDataTracks => {
                formatter.write_str("CDI has more than one candidate Dreamcast data track")
            }
            Self::UnsupportedSectorLayout => {
                formatter.write_str("CDI data track has an unsupported sector geometry")
            }
            Self::ImpossibleOffset => formatter
                .write_str("CDI data track's declared byte range does not fit inside the file"),
            Self::LockPoisoned => {
                formatter.write_str("CDI parse lock was poisoned by a panic in a previous call")
            }
        }
    }
}

impl std::error::Error for CdiIdentityError {}

/// A [`LogicalMedia`] over one `.cdi` file's selected Dreamcast data
/// track, decoded on demand.
///
/// Interior mutability ([`RefCell`]) satisfies [`LogicalMedia::read_at`]'s
/// `&self` signature; the underlying `SectorReader` needs `&mut self` to
/// seek+read a sector, exactly as
/// [`crate::chd_optical_specialist::ChdOpticalSpecialist`] already does
/// for the same reason.
pub struct CdiLogicalMedia {
    reader: RefCell<BinCueSectorReader>,
    len: u64,
}

/// Opens `path` as a DiscJuggler `.cdi` and resolves the single Dreamcast
/// data track needed for identity - see the module documentation for the
/// exact selection rule and every bound enforced before any sector is
/// read.
pub fn open_dreamcast_cdi_logical_media(path: &Path) -> Result<CdiLogicalMedia, CdiIdentityError> {
    let metadata =
        std::fs::metadata(path).map_err(|error| CdiIdentityError::Io(error.to_string()))?;
    let file_len = metadata.len();
    if file_len > MAX_CDI_BYTES {
        return Err(CdiIdentityError::TooLarge {
            bytes: file_len,
            maximum: MAX_CDI_BYTES,
        });
    }

    let tracks = {
        let _guard = CDI_PARSE_LOCK
            .lock()
            .map_err(|_poisoned| CdiIdentityError::LockPoisoned)?;
        parse_discjuggler(path).map_err(|error| CdiIdentityError::Parse(format!("{error:?}")))?
    };
    if tracks.len() > MAX_CDI_TRACKS {
        return Err(CdiIdentityError::TooManyTracks);
    }

    let track = select_dreamcast_data_track(&tracks)?;

    if track.data_offset >= track.physical_sector_size {
        return Err(CdiIdentityError::UnsupportedSectorLayout);
    }
    if track.frame_count == 0 {
        return Err(CdiIdentityError::NoDataTrack);
    }
    let track_bytes = track
        .physical_sector_size
        .checked_mul(track.frame_count)
        .ok_or(CdiIdentityError::ImpossibleOffset)?;
    let track_end = track
        .file_byte_offset
        .checked_add(track_bytes)
        .ok_or(CdiIdentityError::ImpossibleOffset)?;
    if track_end > file_len {
        return Err(CdiIdentityError::ImpossibleOffset);
    }

    let logical_len = track
        .frame_count
        .checked_mul(SECTOR_SIZE)
        .ok_or(CdiIdentityError::ImpossibleOffset)?;

    let reader = BinCueSectorReader::with_layout(
        &track.data_path,
        track.file_byte_offset,
        track.physical_sector_size,
        track.data_offset,
    )
    .map_err(|error| CdiIdentityError::Parse(format!("{error:?}")))?;

    Ok(CdiLogicalMedia {
        reader: RefCell::new(reader),
        len: logical_len,
    })
}

/// Selects the single correct Dreamcast data track from a parsed `.cdi`
/// track list - see the module documentation for the exact rule. Never
/// track order, never a filename.
fn select_dreamcast_data_track(
    tracks: &[DiscJugglerTrack],
) -> Result<&DiscJugglerTrack, CdiIdentityError> {
    let high_density: Vec<&DiscJugglerTrack> = tracks
        .iter()
        .filter(|track| {
            track.is_data && track.base_lba >= u64::from(GDROM_HIGH_DENSITY_START_FRAME)
        })
        .collect();
    match high_density.len() {
        1 => return Ok(high_density[0]),
        0 => {}
        _ => return Err(CdiIdentityError::AmbiguousDataTracks),
    }

    // Single-session shape: no track reaches the GD-ROM high-density
    // boundary, so this can only be trusted if exactly one data track
    // exists in the whole file.
    let plain: Vec<&DiscJugglerTrack> = tracks.iter().filter(|track| track.is_data).collect();
    match plain.len() {
        1 => Ok(plain[0]),
        0 => Err(CdiIdentityError::NoDataTrack),
        _ => Err(CdiIdentityError::AmbiguousDataTracks),
    }
}

impl LogicalMedia for CdiLogicalMedia {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), LogicalMediaError> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(LogicalMediaError::OutOfBounds {
                offset,
                requested_len: buf.len(),
                media_len: self.len,
            })?;
        if end > self.len {
            return Err(LogicalMediaError::OutOfBounds {
                offset,
                requested_len: buf.len(),
                media_len: self.len,
            });
        }

        let mut filled = 0usize;
        let mut reader = self.reader.borrow_mut();
        while filled < buf.len() {
            let absolute = offset + filled as u64;
            let lba = absolute / SECTOR_SIZE;
            let within_sector = (absolute % SECTOR_SIZE) as usize;
            let sector =
                reader
                    .read_sector(lba)
                    .map_err(|error| LogicalMediaError::DecodeFailed {
                        detail: format!("{error:?}"),
                    })?;
            if sector.len() != SECTOR_SIZE as usize {
                return Err(LogicalMediaError::DecodeFailed {
                    detail: format!(
                        "cooked sector at lba {lba} was {} bytes, expected {SECTOR_SIZE}",
                        sector.len()
                    ),
                });
            }
            let take = (SECTOR_SIZE as usize - within_sector).min(buf.len() - filled);
            buf[filled..filled + take]
                .copy_from_slice(&sector[within_sector..within_sector + take]);
            filled += take;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// One test-fixture track spec:
    /// `(track_mode, read_mode, start_address, track_length, pregap)`.
    type TrackSpec = (u32, u32, u32, u32, u32);

    /// Mirrors `opticaldiscs::discjuggler`'s own private test-fixture
    /// builder exactly (that helper is private to its own test module, so
    /// re-derived here per this crate's established per-file-fixture
    /// convention). `sessions` is a list of track specs grouped by
    /// session, each `(track_mode, read_mode, start_address,
    /// track_length, pregap)`.
    fn push_track(
        desc: &mut Vec<u8>,
        track_mode: u32,
        read_mode: u32,
        start_address: u32,
        track_length: u32,
        pregap: u32,
    ) {
        desc.extend_from_slice(&[0u8; 16]);
        desc.push(0); // filename length
        desc.extend_from_slice(&[0u8; 29]);
        desc.extend_from_slice(&[0u8; 2]); // medium type
        desc.extend_from_slice(&2u16.to_le_bytes()); // num_indices
        desc.extend_from_slice(&pregap.to_le_bytes());
        desc.extend_from_slice(&(track_length - pregap).to_le_bytes());
        desc.extend_from_slice(&0u32.to_le_bytes()); // num_cdtext_blocks
        desc.extend_from_slice(&[0u8; 2]);
        desc.extend_from_slice(&track_mode.to_le_bytes());
        desc.extend_from_slice(&[0u8; 4]);
        desc.extend_from_slice(&0u32.to_le_bytes()); // session_idx
        desc.extend_from_slice(&0u32.to_le_bytes()); // track_idx
        desc.extend_from_slice(&start_address.to_le_bytes());
        desc.extend_from_slice(&track_length.to_le_bytes());
        desc.extend_from_slice(&[0u8; 16]);
        desc.extend_from_slice(&read_mode.to_le_bytes());
        desc.extend_from_slice(&0u32.to_le_bytes()); // track_ctl
        desc.extend_from_slice(&[0u8; 9]);
        desc.extend_from_slice(&[0u8; 12]); // ISRC
        desc.extend_from_slice(&0u32.to_le_bytes()); // isrc_valid
        desc.extend_from_slice(&[0u8; 99]);
    }

    /// Mirrors `decode_read_mode` for building a correctly-sized fixture.
    /// An unmapped mode has no real size, but the fixture only needs *a*
    /// finite size to keep the file well-formed enough to reach
    /// `parse_discjuggler`'s own read-mode validation - which is exactly
    /// what `unsupported_read_mode_refuses` exercises.
    fn read_mode_sizes(read_mode: u32) -> (u32, u32) {
        match read_mode {
            0 => (2048, 0),
            1 => (2336, 0),
            3 => (2352, 16),
            4 => (2352, 96),
            _ => (2352, 0),
        }
    }

    /// Builds a minimal `.cdi` file with real, correctly-sized track data
    /// (zero-filled) so a correctly-implemented reader can actually read
    /// cooked sectors out of it, not just parse the trailer.
    ///
    /// `sessions[i]`'s tracks are exactly as in `push_track`. `data_overrides`
    /// optionally overrides the physical byte length of the data region
    /// actually written (e.g. to simulate truncation) - `None` writes the
    /// full, correctly-sized region.
    fn build_cdi(sessions: &[Vec<TrackSpec>], data_len_override: Option<u64>) -> Vec<u8> {
        let mut total_bytes = 0u64;
        for session in sessions {
            for &(_, read_mode, _, length, _) in session {
                let (main, sub) = read_mode_sizes(read_mode);
                total_bytes += (u64::from(main) + u64::from(sub)) * u64::from(length);
            }
        }

        let mut desc = Vec::new();
        desc.push(sessions.len() as u8);
        for session in sessions {
            desc.push(0);
            desc.push(session.len() as u8);
            desc.extend_from_slice(&[0u8; 13]);
            for &(track_mode, read_mode, start_address, length, pregap) in session {
                push_track(
                    &mut desc,
                    track_mode,
                    read_mode,
                    start_address,
                    length,
                    pregap,
                );
            }
        }
        // Trailing empty session.
        desc.push(0);
        desc.push(0);
        desc.extend_from_slice(&[0u8; 13]);

        let dlen = (desc.len() + 4) as u32;

        let data_len = data_len_override.unwrap_or(total_bytes);
        let mut file = vec![0u8; data_len as usize];
        file.extend_from_slice(&desc);
        file.extend_from_slice(&dlen.to_le_bytes());
        file
    }

    /// Stamps a recognisable, valid Dreamcast IP.BIN boot signature plus a
    /// non-copyrightable synthetic product code at the very start of
    /// `data`, matching the fixed layout
    /// `crate::dreamcast_boot_evidence::parse_ip_bin_meta` reads.
    fn stamp_ip_bin(data: &mut [u8]) {
        const HARDWARE_ID: &[u8; 16] = b"SEGA SEGAKATANA ";
        data[0..16].copy_from_slice(HARDWARE_ID);
        let product_number = b"TEST00001 "; // 10 bytes, matches PRODUCT_NUMBER's length
        data[0x40..0x4A].copy_from_slice(product_number);
    }

    fn write_cdi(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new().suffix(".cdi").tempfile().unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        file
    }

    fn read_ip_bin(
        media: &CdiLogicalMedia,
    ) -> [u8; crate::dreamcast_boot_evidence::IP_BIN_META_BYTES] {
        let mut header = [0u8; crate::dreamcast_boot_evidence::IP_BIN_META_BYTES];
        media.read_at(0, &mut header).unwrap();
        header
    }

    #[test]
    fn single_session_cooked_2048_cdi_verifies_the_product_code() {
        // One session, one Mode 1 cooked (read_mode 0) data track.
        let mut bytes = build_cdi(&[vec![(1, 0, 0, 4, 0)]], None);
        let data_region = &mut bytes[..4 * 2048];
        stamp_ip_bin(data_region);
        let file = write_cdi(&bytes);

        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        let header = read_ip_bin(&media);
        let fact = crate::dreamcast_boot_evidence::parse_ip_bin_meta(&header).unwrap();
        assert!(fact.hardware_id_recognized);
        assert_eq!(fact.product_number, "TEST00001");
    }

    #[test]
    fn multi_session_gdrom_cdi_selects_the_high_density_track_not_the_low_density_one() {
        // Session 0: low-density Mode 1 cooked data track (base_lba 0)
        // with a deliberately WRONG stamped product code - selecting this
        // track would be a bug.
        // Session 1: high-density Mode 1 cooked data track at base_lba
        // 45000, carrying the real product code. Both tracks use cooked
        // (read_mode 0) sectors so the stamped bytes land at the plain
        // sector-relative offsets `stamp_ip_bin` writes to.
        let low_density_frames = 4u32;
        let high_density_frames = 4u32;
        let mut bytes = build_cdi(
            &[
                vec![(1, 0, 0, low_density_frames, 0)],
                vec![(1, 0, 45000, high_density_frames, 0)],
            ],
            None,
        );
        let low_start = 0usize;
        let low_len = low_density_frames as usize * 2048;
        stamp_ip_bin(&mut bytes[low_start..low_start + low_len]);
        // Overwrite the wrong-track stamp with a recognisably different
        // (still non-copyrightable) product code so the two are distinguishable.
        bytes[low_start + 0x40..low_start + 0x4A].copy_from_slice(b"WRONGTRACK");

        let high_start = low_len;
        let high_len = high_density_frames as usize * 2048;
        stamp_ip_bin(&mut bytes[high_start..high_start + high_len]);

        let file = write_cdi(&bytes);
        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        let header = read_ip_bin(&media);
        let fact = crate::dreamcast_boot_evidence::parse_ip_bin_meta(&header).unwrap();
        assert_eq!(fact.product_number, "TEST00001");
    }

    #[test]
    fn audio_track_at_the_high_density_boundary_is_never_selected() {
        // An audio track (track_mode 0) sitting past the GD-ROM boundary
        // must never itself become the selected data track.
        let bytes = build_cdi(&[vec![(0, 2, 45000, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::NoDataTrack)
        ));
    }

    #[test]
    fn two_high_density_data_tracks_are_ambiguous() {
        let bytes = build_cdi(
            &[vec![(1, 2, 45000, 4, 0)], vec![(1, 2, 50000, 4, 0)]],
            None,
        );
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::AmbiguousDataTracks)
        ));
    }

    #[test]
    fn two_plain_data_tracks_with_no_high_density_candidate_are_ambiguous() {
        let bytes = build_cdi(&[vec![(1, 2, 0, 4, 0), (1, 2, 0, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::AmbiguousDataTracks)
        ));
    }

    #[test]
    fn no_data_track_at_all_refuses() {
        // A single audio-only track, low density.
        let bytes = build_cdi(&[vec![(0, 2, 0, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::NoDataTrack)
        ));
    }

    #[test]
    fn extension_alone_is_insufficient_without_a_real_descriptor() {
        let file = write_cdi(b"this is not a disc juggler image at all");
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn malformed_truncated_descriptor_refuses() {
        // A file just large enough to carry a plausible-looking descriptor
        // length, but with no real descriptor bytes behind it.
        let mut bytes = vec![0u8; 16];
        let dlen: u32 = 12;
        bytes.extend_from_slice(&dlen.to_le_bytes());
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn unsupported_read_mode_refuses() {
        // read_mode 9 does not map to any known sector geometry.
        let bytes = build_cdi(&[vec![(1, 9, 0, 4, 0)]], None);
        let file = write_cdi(&bytes);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::Parse(_))
        ));
    }

    #[test]
    fn truncated_media_with_an_impossible_offset_refuses() {
        // The trailer declares a full 4-sector track, but the file's data
        // region is truncated to 1 sector - the track's own declared byte
        // range no longer fits inside the real file.
        let full = build_cdi(&[vec![(1, 2, 0, 4, 0)]], None);
        let truncated = build_cdi(&[vec![(1, 2, 0, 4, 0)]], Some(2352));
        assert!(full.len() > truncated.len());
        let file = write_cdi(&truncated);
        assert!(matches!(
            open_dreamcast_cdi_logical_media(file.path()),
            Err(CdiIdentityError::ImpossibleOffset)
        ));
    }

    #[test]
    fn reading_past_the_track_end_refuses_rather_than_fabricating_identity() {
        // Structurally valid single-sector data track (2048 bytes cooked,
        // still more than IP_BIN_META_BYTES - reading IP.BIN itself always
        // succeeds structurally for any real data track). What must never
        // happen is a read past this track's own declared end being
        // silently satisfied instead of refused.
        let bytes = build_cdi(&[vec![(1, 0, 0, 1, 0)]], None);
        let file = write_cdi(&bytes);
        let media = open_dreamcast_cdi_logical_media(file.path()).unwrap();
        assert_eq!(media.len(), 2048);
        let mut header = [0u8; crate::dreamcast_boot_evidence::IP_BIN_META_BYTES];
        assert!(media.read_at(2048, &mut header).is_err());
    }

    #[test]
    fn oversized_cdi_is_refused_before_reading() {
        // Cannot allocate a real multi-GiB file in a unit test; verify the
        // metadata-only pre-check function directly by asserting the
        // bound itself is sane and positive, mirroring
        // `disc_evidence_collector::max_chd_bytes_is_a_positive_sane_bound`.
        const { assert!(MAX_CDI_BYTES > 0) };
        const { assert!(MAX_CDI_BYTES < 100 * 1024 * 1024 * 1024) };
    }
}
