//! Read exact Flycast game IDs through existing logical-media readers. Core
//! catalogue product evidence trims both ends; Flycast only trims the end.
use super::*;
use crate::logical_media::LogicalMedia;
use crate::raw_cd_logical_media::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlycastGameEvidence {
    pub product: String,
    pub metadata_sha256: String,
    pub version: String,
    pub device_info: String,
}
pub fn inspect_flycast_game(game: &Path) -> Checked<FlycastGameEvidence> {
    safe_path(game)?;
    let extension = game
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let media: Box<dyn LogicalMedia> = match extension.as_str() {
        "gdi"=>{
            use crate::ingestion::gdi::{resolve_gdi_data_track,GdiDataTrackMode};
            let track=resolve_gdi_data_track(game).map_err(|e|e.to_string())?;safe_path(&track.path)?;
            match track.mode {
                GdiDataTrackMode::Cooked2048=>Box::new(open_cooked_cd_file_logical_media(&track.path).map_err(|e|e.to_string())?),
                GdiDataTrackMode::Raw2352=>Box::new(open_raw_cd_file_logical_media(&track.path).map_err(|e|e.to_string())?),
            }
        },
        "cue"=>{
            use crate::ingestion::cue_bin::{resolve_data_track,CueDataTrackMode};
            let track=resolve_data_track(game).map_err(|e|e.to_string())?;safe_path(&track.path)?;
            match track.mode {
                CueDataTrackMode::Mode1_2048=>Box::new(open_cooked_cd_file_logical_media_range(&track.path,track.index_01_frame,track.data_frame_count).map_err(|e|e.to_string())?),
                CueDataTrackMode::Mode1_2352=>Box::new(open_raw_cd_file_logical_media_range(&track.path,track.index_01_frame,track.data_frame_count).map_err(|e|e.to_string())?),
                _=>return Err("Unsupported Dreamcast CUE sector mapping".into()),
            }
        },
        "iso"=>Box::new(open_cooked_cd_file_logical_media(game).map_err(|e|e.to_string())?),
        #[cfg(feature="dreamcast-cdi")]
        "cdi"=>Box::new(crate::dreamcast_cdi::open_dreamcast_cdi_logical_media(game).map_err(|e|e.to_string())?),
        #[cfg(feature="chd-optical-specialist")]
        "chd"=>Box::new(crate::chd_optical_specialist::open_chd_optical_specialist(game).map_err(|e|e.to_string())?),
        _=>return Err("Exact Dreamcast boot metadata cannot be read for this format/build; installation refused".into()),
    };
    let mut header = [0u8; crate::dreamcast_boot_evidence::IP_BIN_META_BYTES];
    media.read_at(0, &mut header).map_err(|e| e.to_string())?;
    evidence(&header)
}
fn evidence(header: &[u8]) -> Checked<FlycastGameEvidence> {
    let metadata = crate::dreamcast_boot_evidence::parse_ip_bin_meta(header)
        .ok_or("Dreamcast boot metadata truncated")?;
    if !metadata.hardware_id_recognized {
        return Err("Unrecognised Dreamcast boot metadata".into());
    }
    let raw = &header[0x40..0x4a];
    // Upstream trims the complete field before its c_str() NUL truncation.
    // Refuse NUL-bearing fields rather than silently normalising a different key.
    if raw.contains(&0) {
        return Err(
            "NUL-bearing Flycast product identity is unsupported; exact mapping refused".into(),
        );
    }
    let product = std::str::from_utf8(raw)
        .map_err(|e| e.to_string())?
        .trim_end_matches(|c: char| c.is_ascii_whitespace())
        .to_string();
    if product != metadata.product_number || product.is_empty() {
        return Err("Catalogue identity normalisation differs from Flycast's exact game ID; mapping refused".into());
    }
    Ok(FlycastGameEvidence {
        product,
        metadata_sha256: digest(header),
        version: metadata.product_version,
        device_info: metadata.device_info,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_game_key_is_not_uppercased_or_hyphenated_and_leading_space_is_refused() {
        let mut header = [b' '; 256];
        header[..16].copy_from_slice(b"SEGA SEGAKATANA ");
        header[0x40..0x4a].copy_from_slice(b"t-TEST    ");
        assert_eq!(evidence(&header).unwrap().product, "t-TEST");
        header[0x40..0x4a].copy_from_slice(b" T-TEST   ");
        assert!(evidence(&header).is_err());
        header[0x40..0x4a].copy_from_slice(b"T-TEST\0xxx");
        assert!(evidence(&header).is_err());
        header[0x40..0x4a].copy_from_slice(b"T-TEST \0  ");
        assert!(evidence(&header).is_err());
    }
}
