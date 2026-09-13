use super::{model::*, naming::filename_evidence};
use crate::{
    dat::library_identity_summary::{DatProvenanceFreshness, LibraryDatIdentitySummary},
    game_identity::{GameIdentityReport, IdentityKind, IdentityStatus},
};
use std::path::Path;

pub fn family_for_format(format: &str) -> Option<MediaFamily> {
    match format.to_ascii_lowercase().as_str() {
        "cue" | "bin" | "chd" | "gdi" | "cdi" | "iso" | "gcm" | "rvz" | "wia" | "wbfs" => {
            Some(MediaFamily::Optical)
        }
        "adf" | "adz" | "ipf" | "dms" | "st" | "msa" | "stx" | "dsk" | "d64" | "g64" | "nib"
        | "woz" | "2mg" | "do" | "po" | "d88" | "d77" | "fdi" | "xdf" | "dim" | "fds" | "trd"
        | "scl" | "ssd" | "dsd" | "dc42" | "d81" => Some(MediaFamily::Floppy),
        "tap" | "tzx" | "t64" | "cdt" | "cas" | "uef" | "wav" => Some(MediaFamily::Tape),
        _ => None,
    }
}
pub fn media_record(path: impl AsRef<Path>, platform: Option<&str>) -> MediaRecord {
    let path = path.as_ref();
    let format = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let family = family_for_format(&format);
    let mut evidence = Vec::new();
    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
        evidence.push(filename_evidence(name, family));
    }
    if let Some(parent) = path.parent() {
        let mut e = MediaEvidence::new(EvidenceKind::Directory, parent.to_string_lossy());
        e.notes
            .push("Directory proximity is supporting evidence only".into());
        evidence.push(e);
    }
    MediaRecord {
        source: MediaSource {
            path: path.to_owned(),
            archive_member: None,
        },
        platform: platform.map(str::to_owned),
        family,
        format,
        availability: MediaAvailability::Unverified,
        evidence,
        warnings: Vec::new(),
    }
}
/// Current verified DAT summaries only. No DAT loading, indexing or authority mutation.
pub fn attach_dat_identity(record: &mut MediaRecord, summary: &LibraryDatIdentitySummary) {
    if !summary.is_verified() || summary.provenance_freshness != DatProvenanceFreshness::Current {
        record.warnings.push("DAT identity is not a current single cryptographic match; no trusted topology claim added".into());
        return;
    }
    let Some(name) = &summary.canonical.canonical_dat_name else {
        return;
    };
    let mut e = filename_evidence(name, record.family);
    e.provenance = Provenance {
        kind: EvidenceKind::TrustedDat,
        source: summary.source.source_id.clone(),
        version: summary.source.source_revision.clone(),
    };
    let namespace = format!(
        "dat:{}:{}",
        summary.source.source_id,
        summary
            .source
            .source_revision
            .as_deref()
            .unwrap_or("unspecified")
    );
    if let Some(title) = &e.title
        && !title.is_empty()
    {
        e.release = Some(IdentityKey::new(namespace.clone(), title));
    }
    e.medium = Some(IdentityKey::new(
        namespace,
        format!(
            "{name}::{}",
            summary
                .canonical
                .canonical_rom_name
                .as_deref()
                .unwrap_or("")
        ),
    ));
    e.equivalence = Equivalence::AuthorityMapping;
    if let Some(region) = &summary.canonical.region {
        e.variant.region = Some(region.to_ascii_lowercase());
    }
    if let Some(revision) = &summary.canonical.revision {
        e.variant.revision = Some(revision.to_ascii_lowercase());
    }
    e.notes.push("Topology tokens come from the matched DAT entry; individual ROM hash identity is not a catalogue-wide set audit".into());
    record.evidence.push(e);
}
/// Richer authority producers may supply membership without changing this engine
/// or existing DAT APIs. The producer must bind claims to its current scan/version.
pub trait MediaTopologyAuthority {
    fn evidence_for(&self, source: &MediaSource) -> Vec<MediaEvidence>;
}
pub fn attach_authority(record: &mut MediaRecord, authority: &impl MediaTopologyAuthority) {
    record
        .evidence
        .extend(authority.evidence_for(&record.source));
}

pub fn attach_native_identity(record: &mut MediaRecord, report: &GameIdentityReport) {
    if report.archive_path != record.source.path {
        record
            .warnings
            .push("Native report belongs to another source; ignored".into());
        return;
    }
    let supplied =
        crate::game_identity::IdentityPlatform::from_catalogue(record.platform.as_deref());
    if record.platform.is_some()
        && supplied != report.platform
        && report
            .evidence
            .iter()
            .any(|e| e.status == IdentityStatus::Verified)
    {
        let mut e = MediaEvidence::new(EvidenceKind::VerifiedNative, "game_identity platform");
        e.notes
            .push("Conflicting supplied platform and native identity report".into());
        record.evidence.push(e);
        return;
    }
    for fact in &report.evidence {
        if fact.status != IdentityStatus::Verified {
            continue;
        }
        if record.source.archive_member.as_ref().map(|m| m.index) != fact.provenance.member_index {
            continue;
        }
        let Some(value) = &fact.value else {
            continue;
        };
        let mut e = MediaEvidence::new(
            EvidenceKind::VerifiedNative,
            format!("game_identity:{}", fact.provenance.method),
        );
        match fact.kind {
            IdentityKind::DolphinGameId => {
                e.release = Some(IdentityKey::new("dolphin-game-id", value));
                e.notes.push(
                    "Game ID identifies the product; revision and disc number remain separate"
                        .into(),
                );
            }
            IdentityKind::DolphinDiscNumber => {
                if let Some(number) = value.parse::<u16>().ok().and_then(|n| n.checked_add(1)) {
                    e.ordinal = Some(MediaOrdinal {
                        number,
                        unit: OrdinalUnit::Disc,
                    });
                }
            }
            IdentityKind::DolphinRevision => e.variant.revision = Some(value.to_ascii_lowercase()),
            IdentityKind::DolphinRegion => {
                e.notes.push(format!("Native region byte {value}"));
                // The native reader deliberately exposes a raw byte, not a locale.
                // Reuse the existing product-region mapping; do not compare E to USA.
                e.variant.region = report
                    .verified_dolphin_game_id()
                    .and_then(crate::patch_manager::region_for_game_id)
                    .map(|region| region.display_name().to_ascii_lowercase());
            }
            IdentityKind::Ps1Serial
            | IdentityKind::Ps2Serial
            | IdentityKind::Pcsx2ExecutableCrc
            | IdentityKind::SaturnProductNumber
            | IdentityKind::DreamcastProductCode
            | IdentityKind::SegaCdProductCode
            | IdentityKind::PspDiscId
            | IdentityKind::ThreeDoDiscId
            | IdentityKind::PcfxDiscHash => {
                e.medium = Some(IdentityKey::new(format!("native:{:?}", fact.kind), value));
                e.notes.push("Product/executable identity is not proof of cross-format content equivalence or a shared multi-medium release".into());
            }
            _ => continue,
        }
        record.evidence.push(e);
    }
    if let (Some(game), Some(disc)) = (
        report.verified_dolphin_game_id(),
        report.verified_value(IdentityKind::DolphinDiscNumber),
    ) {
        let mut e = MediaEvidence::new(
            EvidenceKind::VerifiedNative,
            "Dolphin game/disc/revision fields",
        );
        e.medium = Some(IdentityKey::new(
            "dolphin-medium",
            format!(
                "{game}:{disc}:{}",
                report
                    .verified_value(IdentityKind::DolphinRevision)
                    .unwrap_or("unknown")
            ),
        ));
        record.evidence.push(e);
    }
    record.warnings.extend(report.warnings.clone());
}
pub fn attach_optical_fingerprint(
    record: &mut MediaRecord,
    fingerprint: &crate::optical_fingerprint::CanonicalOpticalFingerprint,
) {
    if fingerprint.source != record.source.path {
        record
            .warnings
            .push("Optical fingerprint belongs to another source; ignored".into());
        return;
    }
    let mut e = MediaEvidence::new(
        EvidenceKind::VerifiedNative,
        "existing canonical optical fingerprint",
    );
    e.provenance.version = Some(fingerprint.schema.into());
    e.medium = Some(IdentityKey::new(
        fingerprint.schema,
        format!(
            "{}:{:?}",
            fingerprint.canonical_sha256, fingerprint.structure
        ),
    ));
    e.equivalence = Equivalence::CanonicalContent;
    record.evidence.push(e);
}
pub fn attach_disk_evidence(
    record: &mut MediaRecord,
    disk: &crate::disk_format::DiskFormatEvidence,
) {
    use crate::disk_format::DiskFormatMetadata;
    let mut e = MediaEvidence::new(
        EvidenceKind::Embedded,
        "disk_format bounded structural inspection",
    );
    if let Some(platform) = disk.platform
        && disk.conclusive
    {
        if let Some(current) = &record.platform
            && crate::platform::platform_by_id(current)
                .or_else(|| crate::platform::platform_for_alias(current))
                .map(|p| p.id)
                != Some(platform)
        {
            e.notes.push(format!(
                "Conflicting native platform {platform} and supplied {current}"
            ));
        } else {
            record.platform = Some(platform.into());
        }
    }
    e.notes.extend(disk.evidence.clone());
    match &disk.metadata {
        Some(DiskFormatMetadata::D64(layout)) => {
            e.notes.push(format!("Disk label {:?}; disk ID {:?}; {} directory entries; labels are not release identity",layout.disk_name,layout.disk_id,layout.directory.len()));
            e.notes
                .push("A D64 does not establish the physical flippy-disk side topology".into());
        }
        Some(DiskFormatMetadata::Floppy(g)) => {
            e.side_layout = Some(SideLayout::WholeMedium);
            e.expected_sides = (1..=g.sides.min(2) as u8)
                .map(|number| MediaSide { number })
                .collect();
        }
        Some(DiskFormatMetadata::Dsk(g)) => {
            e.side_layout = Some(SideLayout::WholeMedium);
            e.expected_sides = (1..=g.declared_sides.min(2))
                .map(|number| MediaSide { number })
                .collect();
        }
        Some(DiskFormatMetadata::Fds(g)) => {
            e.side_layout = Some(SideLayout::WholeMedium);
            e.expected_sides = (1..=g.sides.min(2))
                .map(|number| MediaSide { number })
                .collect();
        }
        _ => {}
    }
    if let Some(refusal) = &disk.refusal {
        record.warnings.push(format!(
            "Floppy native inspection did not prove identity: {refusal:?}"
        ));
    }
    record.evidence.push(e);
}
pub fn attach_tape_analysis(
    record: &mut MediaRecord,
    analysis: &crate::tape_analysis::TapeAnalysis,
) {
    let mut e = MediaEvidence::new(EvidenceKind::Embedded, "existing deep tape analysis");
    e.notes.push(format!(
        "Analyzer platform hint {:?}; shared tape formats still require platform context",
        analysis.platform
    ));
    e.notes.push(format!(
        "Format {:?}; {} blocks; {} entries; checksum {:?}",
        analysis.format,
        analysis.block_count,
        analysis.entries.len(),
        analysis.checksum
    ));
    for entry in &analysis.entries {
        e.notes.push(format!(
            "Entry {:?}: {:?}, address {:?}, length {}, checksum {:?}",
            entry.name, entry.kind, entry.load_address, entry.length, entry.checksum
        ));
    }
    e.notes.extend(analysis.metadata.clone());
    if let Some(loader) = &analysis.loader {
        e.notes.push(format!(
            "Loader {:?} ({:?}); clues {:?}; loader identity is not release identity",
            loader.class, loader.confidence, loader.clues
        ));
    }
    if matches!(
        analysis.format,
        crate::tape_analysis::TapeFormat::CommodoreTap
    ) {
        e.notes
            .push("Pulse-only TAP exposes no file directory or game title".into());
    }
    if analysis.checksum == crate::tape_analysis::ChecksumState::Invalid {
        e.notes.push("Conflicting tape checksum evidence".into());
    }
    record.warnings.extend(analysis.warnings.clone());
    record.evidence.push(e);
}
/// Reuses the existing catalogue hierarchy without retaining or invoking any
/// organisation/rename action. Archive-member evidence can be attached separately.
pub fn from_library_input(
    input: &crate::platform_evidence_fusion::library_planning::LibraryPlanInput,
    generation: u64,
) -> MediaRecord {
    use crate::platform_evidence_fusion::{
        library_grouping::{SetMembership, hierarchy_for},
        library_planning::identity_result_to_resolution,
    };
    let resolution = identity_result_to_resolution(&input.identity, generation);
    let mut r = media_record(&input.source_path, resolution.platform());
    let hierarchy = hierarchy_for(
        &input.identity,
        resolution.platform(),
        input
            .source_path
            .file_name()
            .and_then(|x| x.to_str())
            .unwrap_or(""),
        None,
    );
    if let SetMembership::MultiDiscPart {
        base_title,
        part,
        total,
    } = hierarchy.set
    {
        let mut e = MediaEvidence::new(
            EvidenceKind::Metadata,
            "existing library hierarchy; freshness must be supplied separately",
        );
        e.title = Some(base_title);
        e.ordinal = Some(MediaOrdinal {
            number: part,
            unit: OrdinalUnit::Disc,
        });
        e.expected_count = Some(ExpectedCount {
            count: total,
            unit: OrdinalUnit::Disc,
        });
        r.evidence.push(e);
    }
    if input.identity.has_conflict() {
        r.warnings
            .push("Catalogue identity has unresolved conflicts".into());
        let mut e = MediaEvidence::new(EvidenceKind::Metadata, "catalogue identity");
        e.notes.push("Conflicting catalogue identity".into());
        r.evidence.push(e);
    }
    r
}

pub fn attach_amiga_floppy(
    record: &mut MediaRecord,
    inspection: &crate::amiga_disk::AmigaFloppyInspection,
) {
    let mut e = MediaEvidence::new(EvidenceKind::Embedded, "existing Amiga OFS/FFS inspection");
    e.side_layout = Some(SideLayout::WholeMedium);
    e.notes.push(format!(
        "Validated {:?}; volume label {:?}; the label is descriptive, not a release ID",
        inspection.filesystem.family, inspection.filesystem.volume_label
    ));
    if record
        .platform
        .as_deref()
        .and_then(|s| {
            crate::platform::platform_by_id(s).or_else(|| crate::platform::platform_for_alias(s))
        })
        .is_some_and(|p| p.id != "Amiga")
    {
        e.notes
            .push("Conflicting supplied platform and validated Amiga filesystem".into());
    } else {
        record.platform = Some("Amiga".into());
    }
    record.evidence.push(e);
}

pub fn profile_with_readiness(
    mut profile: MediaProfile,
    readiness: crate::launch::readiness::LaunchReadiness,
) -> MediaProfile {
    profile.readiness_hint = Some(format!("{readiness:?}"));
    profile
}
