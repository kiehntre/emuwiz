//! Bounded Sharp X68000 XDF and DIM floppy-image evidence.
//!
//! XDF is a headerless raw image, so its extension and capacity are not
//! sufficient. The accepted XDF form is the documented 2HD image whose first
//! sector contains the X68000 IPL/BPB fields and whose length is exactly
//! 77 cylinders x 2 sides x 8 sectors x 1024 bytes. DIM is a DIFC container:
//! its fixed header describes which physical tracks are present, followed by
//! only those tracks in C/H/S order.
//!
//! Format details are cross-checked against the [pc98.org DIM documentation],
//! XEiJ's [FDMedia implementation], and the independent [XDF builder
//! implementation].
//!
//! [pc98.org DIM documentation]: https://www.pc98.org/project/doc/dim.html
//! [FDMedia implementation]: https://stdkmd.net/xeij/source/xeij-FDMedia.java.htm
//! [XDF builder implementation]: https://github.com/mikewolak/x68k_sprite_demo/blob/main/README.md

use std::sync::atomic::AtomicBool;

use super::{
    BoundedReader, DiskFormat, DiskFormatContext, DiskFormatEvidence, DiskFormatMetadata,
    DiskFormatRefusal, X68000Layout, confidence_for,
};

const DIM_HEADER_BYTES: u64 = 0x100;
const DIM_TRACK_FLAGS: usize = 160;
const DIM_SIGNATURE_OFFSET: usize = 0xab;
const DIM_SIGNATURE: &[u8; 13] = b"DIFC HEADER  ";
const XDF_BYTES: u64 = 77 * 2 * 8 * 1024;

#[derive(Clone, Copy)]
struct Geometry {
    name: &'static str,
    bytes_per_sector: u16,
    sectors_per_track: u16,
    cylinders: u16,
}

impl Geometry {
    fn tracks(self) -> u16 {
        self.cylinders * 2
    }

    fn bytes_per_track(self) -> u64 {
        u64::from(self.bytes_per_sector) * u64::from(self.sectors_per_track)
    }

    fn payload_bytes(self) -> u64 {
        self.bytes_per_track() * u64::from(self.tracks())
    }
}

fn dim_geometry(media: u8) -> Option<Geometry> {
    // The media-byte mapping follows the independently implemented XEiJ
    // FDMedia table.  0x03 is its 2HDE variant; 0x09 is 2HQ, both also
    // documented as valid 1.44MB DIM variants by pc98.org.
    Some(match media {
        0x00 => Geometry {
            name: "2HD",
            bytes_per_sector: 1024,
            sectors_per_track: 8,
            cylinders: 77,
        },
        0x01 => Geometry {
            name: "2HS",
            bytes_per_sector: 1024,
            sectors_per_track: 9,
            cylinders: 80,
        },
        0x02 => Geometry {
            name: "2HC",
            bytes_per_sector: 512,
            sectors_per_track: 15,
            cylinders: 80,
        },
        0x03 => Geometry {
            name: "2HDE",
            bytes_per_sector: 1024,
            sectors_per_track: 9,
            cylinders: 80,
        },
        0x09 => Geometry {
            name: "2HQ",
            bytes_per_sector: 512,
            sectors_per_track: 18,
            cylinders: 80,
        },
        _ => return None,
    })
}

pub(super) fn inspect_xdf(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    let result = validate_xdf(reader, cancel).map(|layout| (DiskFormat::X68000Xdf, layout));
    finish(reader, context, result)
}

pub(super) fn inspect_dim(
    reader: &mut BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    cancel: Option<&AtomicBool>,
) -> DiskFormatEvidence {
    let result = validate_dim(reader, cancel).map(|layout| (DiskFormat::X68000Dim, layout));
    finish(reader, context, result)
}

fn finish(
    reader: &BoundedReader<'_>,
    context: DiskFormatContext<'_>,
    result: Result<(DiskFormat, X68000Layout), DiskFormatRefusal>,
) -> DiskFormatEvidence {
    match result {
        Ok((format, layout)) => {
            let (confidence, conclusive) = confidence_for(format, context);
            DiskFormatEvidence {
                format: Some(format),
                platform: Some(format.platform()),
                confidence,
                conclusive,
                evidence: vec![format!(
                    "{} geometry validated: {} cylinders x 2 sides x {} sectors of {} bytes",
                    layout.format_name,
                    layout.cylinders,
                    layout.sectors_per_track,
                    layout.bytes_per_sector
                ), "X68000 floppy structure is valid; internal titles/comments are not identity authority".to_string()],
                bytes_inspected: reader.bytes_read(),
                refusal: None,
                metadata: Some(DiskFormatMetadata::X68000(layout)),
                read_via_symlink: false,
            }
        }
        Err(refusal) => {
            let mut evidence = DiskFormatEvidence::refused(refusal);
            evidence.bytes_inspected = reader.bytes_read();
            evidence
        }
    }
}

fn validate_xdf(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<X68000Layout, DiskFormatRefusal> {
    if reader.len() != XDF_BYTES {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: XDF_BYTES,
            actual_bytes: reader.len(),
        });
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }
    let bpb = reader.read_exact_at(0, 64)?;
    if bpb.first().copied() != Some(0x60) {
        return Err(malformed(
            "XDF boot sector lacks the X68000 IPL branch opcode",
        ));
    }
    let bps = 1024u16.to_le_bytes();
    let reserved = 2u16.to_le_bytes();
    let root_entries = 192u16.to_le_bytes();
    let partition_sectors = 1232u16.to_le_bytes();
    let sectors_per_track = 8u16.to_le_bytes();
    let sides = 2u16.to_le_bytes();
    let expected = [
        (0x0b, bps.as_slice()),
        (0x0d, &[1][..]),
        (0x0e, reserved.as_slice()),
        (0x10, &[2][..]),
        (0x11, root_entries.as_slice()),
        (0x13, partition_sectors.as_slice()),
        (0x15, &[0xfe][..]),
        (0x16, &[2][..]),
        (0x18, sectors_per_track.as_slice()),
        (0x1a, sides.as_slice()),
    ];
    for (offset, bytes) in expected {
        let end = offset + bytes.len();
        if bpb.get(offset..end) != Some(bytes) {
            return Err(malformed(format!(
                "XDF BPB field at {offset:#04x} is inconsistent"
            )));
        }
    }
    Ok(X68000Layout {
        format_name: "XDF 2HD",
        bytes_per_sector: 1024,
        sectors_per_track: 8,
        tracks_per_cylinder: 2,
        cylinders: 77,
        header_bytes: 0,
        stored_tracks: 154,
        payload_bytes: XDF_BYTES,
    })
}

fn validate_dim(
    reader: &mut BoundedReader<'_>,
    cancel: Option<&AtomicBool>,
) -> Result<X68000Layout, DiskFormatRefusal> {
    if reader.len() < DIM_HEADER_BYTES {
        return Err(DiskFormatRefusal::TooSmall {
            length: reader.len(),
            minimum: DIM_HEADER_BYTES,
        });
    }
    let header = reader.read_exact_at(0, DIM_HEADER_BYTES as usize)?;
    if header.get(DIM_SIGNATURE_OFFSET..DIM_SIGNATURE_OFFSET + DIM_SIGNATURE.len())
        != Some(DIM_SIGNATURE.as_slice())
    {
        return Err(malformed("DIM header does not contain the DIFC signature"));
    }
    if header[0xa1..0xab].iter().any(|&byte| byte != 0)
        || header[0xb8..0xfe].iter().any(|&byte| byte != 0)
    {
        return Err(malformed("DIM reserved header bytes are not zero"));
    }
    let geometry = dim_geometry(header[0])
        .ok_or_else(|| malformed(format!("unsupported DIM media byte {:#04x}", header[0])))?;
    let tracks = usize::from(geometry.tracks());
    let declared_tracks = usize::from(header[0xff]);
    if declared_tracks != 0 && declared_tracks != tracks {
        return Err(malformed(format!(
            "DIM track-count byte {declared_tracks} disagrees with {}",
            geometry.tracks()
        )));
    }
    let mut stored_tracks = 0usize;
    for (index, &flag) in header[1..=DIM_TRACK_FLAGS].iter().enumerate() {
        let valid = if index < tracks {
            flag == 0 || flag == 1
        } else {
            flag == 0
        };
        if !valid {
            return Err(malformed(format!(
                "DIM track-presence flag {flag:#04x} at index {index} is invalid"
            )));
        }
        if index < tracks && flag == 1 {
            stored_tracks += 1;
        }
    }
    if super::cancelled(cancel) {
        return Err(DiskFormatRefusal::Cancelled);
    }
    let payload_bytes = geometry.bytes_per_track() * stored_tracks as u64;
    let expected_len = DIM_HEADER_BYTES
        .checked_add(payload_bytes)
        .ok_or_else(|| malformed("DIM length arithmetic overflowed"))?;
    if reader.len() != expected_len {
        return Err(DiskFormatRefusal::GeometryMismatch {
            declared_bytes: expected_len,
            actual_bytes: reader.len(),
        });
    }
    Ok(X68000Layout {
        format_name: geometry.name,
        bytes_per_sector: geometry.bytes_per_sector,
        sectors_per_track: geometry.sectors_per_track,
        tracks_per_cylinder: 2,
        cylinders: geometry.cylinders,
        header_bytes: DIM_HEADER_BYTES,
        stored_tracks: stored_tracks as u16,
        payload_bytes: geometry.payload_bytes(),
    })
}

fn malformed(detail: impl Into<String>) -> DiskFormatRefusal {
    DiskFormatRefusal::Malformed {
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DiskFormat, DiskFormatMetadata, X68000Layout};

    // Parser fixtures are exercised through the crate's existing filesystem
    // integration tests; keeping format construction there avoids a second
    // temporary-file harness and tests the public dispatch path as well.
    #[test]
    fn media_geometry_arithmetic_is_bounded_and_exact() {
        assert_eq!(super::dim_geometry(0).unwrap().payload_bytes(), 1_261_568);
        assert_eq!(super::dim_geometry(2).unwrap().payload_bytes(), 1_228_800);
        assert!(super::dim_geometry(0xff).is_none());
    }

    #[test]
    fn xdf_requires_the_documented_boot_bpb_and_geometry() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disk.xdf");
        let mut image = vec![0; super::XDF_BYTES as usize];
        image[0] = 0x60;
        image[0x0b..0x0d].copy_from_slice(&1024u16.to_le_bytes());
        image[0x0d] = 1;
        image[0x0e..0x10].copy_from_slice(&2u16.to_le_bytes());
        image[0x10] = 2;
        image[0x11..0x13].copy_from_slice(&192u16.to_le_bytes());
        image[0x13..0x15].copy_from_slice(&1232u16.to_le_bytes());
        image[0x15] = 0xfe;
        image[0x16] = 2;
        image[0x18..0x1a].copy_from_slice(&8u16.to_le_bytes());
        image[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        fs::write(&path, &image).unwrap();
        let evidence = super::super::inspect_disk_format(
            &path,
            &crate::safe_read::TrustedRoots::none(),
            super::super::DiskFormatContext::default(),
            None,
        );
        assert_eq!(evidence.format, Some(DiskFormat::X68000Xdf));
        assert!(evidence.conclusive);

        let random = dir.path().join("random.xdf");
        fs::write(&random, vec![0xa5; super::XDF_BYTES as usize]).unwrap();
        assert!(
            super::super::inspect_disk_format(
                &random,
                &crate::safe_read::TrustedRoots::none(),
                super::super::DiskFormatContext::default(),
                None,
            )
            .format
            .is_none()
        );
    }

    #[test]
    fn dim_validates_signature_flags_geometry_and_payload_length() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("disk.dim");
        let mut image = vec![0; 0x100 + 1024 * 8];
        image[0] = 0;
        image[1] = 1;
        image[0xab..0xb8].copy_from_slice(super::DIM_SIGNATURE);
        fs::write(&path, &image).unwrap();
        let evidence = super::super::inspect_disk_format(
            &path,
            &crate::safe_read::TrustedRoots::none(),
            super::super::DiskFormatContext::default(),
            None,
        );
        assert_eq!(evidence.format, Some(DiskFormat::X68000Dim));
        assert_eq!(
            evidence.metadata.as_ref().unwrap().clone(),
            DiskFormatMetadata::X68000(X68000Layout {
                format_name: "2HD",
                bytes_per_sector: 1024,
                sectors_per_track: 8,
                tracks_per_cylinder: 2,
                cylinders: 77,
                header_bytes: 0x100,
                stored_tracks: 1,
                payload_bytes: 1_261_568,
            })
        );

        image[0xab] = b'X';
        fs::write(&path, &image).unwrap();
        assert!(
            super::super::inspect_disk_format(
                &path,
                &crate::safe_read::TrustedRoots::none(),
                super::super::DiskFormatContext::default(),
                None,
            )
            .format
            .is_none()
        );
    }

    #[test]
    fn direct_discovery_accepts_valid_xdf_and_folder_corroborates_x68000() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let platform_dir = dir.path().join("X68000");
        fs::create_dir(&platform_dir).unwrap();
        let path = platform_dir.join("game.xdf");
        let mut image = vec![0; super::XDF_BYTES as usize];
        image[0] = 0x60;
        image[0x0b..0x0d].copy_from_slice(&1024u16.to_le_bytes());
        image[0x0d] = 1;
        image[0x0e..0x10].copy_from_slice(&2u16.to_le_bytes());
        image[0x10] = 2;
        image[0x11..0x13].copy_from_slice(&192u16.to_le_bytes());
        image[0x13..0x15].copy_from_slice(&1232u16.to_le_bytes());
        image[0x15] = 0xfe;
        image[0x16] = 2;
        image[0x18..0x1a].copy_from_slice(&8u16.to_le_bytes());
        image[0x1a..0x1c].copy_from_slice(&2u16.to_le_bytes());
        fs::write(&path, image).unwrap();

        let report = crate::ingestion::discovery::discover_source(&platform_dir).unwrap();
        assert_eq!(report.items.len(), 1);
        assert_eq!(
            report.items[0].platform_hint.as_deref(),
            Some("Sharp X68000"),
            "{report:?}"
        );
        assert_eq!(
            report.items[0].validation_state,
            crate::ingestion::discovery::ValidationState::Accepted
        );
    }
}
