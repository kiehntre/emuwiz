//! Structural ST/STX classification over the existing bounded readers.
use super::*;
use crate::{
    disk_format::{self, DiskFormat, DiskFormatContext},
    platform::{DetectionConfidence, DetectionRequest, detect_platform_report},
    safe_read::TrustedRoots,
};
pub(super) fn discover(path: &Path, source_root: &Path) -> GameDiscovery {
    let evidence = disk_format::inspect_disk_format(
        path,
        &TrustedRoots::none(),
        DiskFormatContext::default(),
        None,
    );
    let Some(format @ (DiskFormat::AtariStRawFloppy | DiskFormat::AtariStPasti)) = evidence.format
    else {
        let detail = evidence
            .refusal
            .as_ref()
            .map(|r| r.detail())
            .unwrap_or_else(|| "not recognised ST/STX structure".into());
        let reason = if matches!(
            evidence.refusal,
            Some(
                disk_format::DiskFormatRefusal::NotReadable { .. }
                    | disk_format::DiskFormatRefusal::Truncated { .. }
                    | disk_format::DiskFormatRefusal::Cancelled
            )
        ) {
            SkipReason::InspectionUnavailable(detail.clone())
        } else {
            SkipReason::InvalidContent(detail.clone())
        };
        return skipped(
            path.into(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            reason,
            format!(
                "ST/STX structure did not validate: {detail}. A filename/folder does not validate a disk."
            ),
        );
    };
    let mut request = DetectionRequest::new(path, source_root);
    request.read_signatures = true;
    let platform = detect_platform_report(&request);
    if platform.platform != Some("AtariST") || platform.confidence != DetectionConfidence::Confirmed
    {
        return skipped(
            path.into(),
            ContainerKind::DirectFile,
            Some(ContentKind::ComputerDisk),
            SkipReason::AmbiguousPlatform,
            format!(
                "Structurally valid {}, but Atari ST platform evidence is ambiguous or conflicts. Raw ST/FAT12 geometry is shared with DOS; source corroboration is needed. No release identity is verified.",
                format.label()
            ),
        );
    }
    let mut identity = identity_for(path, source_root);
    if let Some(identity) = &mut identity {
        identity.platform = Some("AtariST".into());
    }
    accepted(
        path.into(),
        ContainerKind::DirectFile,
        ContentKind::ComputerDisk,
        identity,
        format!(
            "{}; {} bytes structurally inspected. {} Platform/media evidence only: exact game identity still needs a catalogue match. No sector payload decoding, conversion or disk-set ordering was performed.",
            format.label(),
            evidence.bytes_inspected,
            evidence.evidence.join(". ")
        ),
    )
}
