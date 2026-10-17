//! Worker-only cartridge guidance. It never authorizes an additional transform.
use super::*;
use archivefs_core::dat::audit::KnownFileEvidence;
use archivefs_core::dat::library_identity_summary::{
    DatProvenanceFreshness, DatVerificationState, LibraryDatIdentitySummary,
};
use archivefs_core::rom_representation_diagnostics::{
    DatRepresentationState, RomFormatContext, verify_dat_representation,
};
use archivefs_core::safe_read::{TrustedRoots, open_bounded_read};
use archivefs_core::standalone_patch::{
    PatchRepresentationPreflight, preflight_patch_representation,
};
use std::io::Read;
use std::path::Path;

pub(super) fn format_context(
    platform: archivefs_core::game_identity::IdentityPlatform,
) -> RomFormatContext {
    use archivefs_core::game_identity::IdentityPlatform::*;
    match platform {
        Snes => RomFormatContext::Snes,
        Nes => RomFormatContext::Nes,
        N64 => RomFormatContext::N64,
        MegaDrive => RomFormatContext::Genesis,
        _ => RomFormatContext::Unknown,
    }
}

pub(super) fn read_base(path: &Path) -> Result<Vec<u8>, String> {
    let source = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.detail())?;
    if source.len() > MAX_APPLY_BYTES {
        return Err("Selected source exceeds the patch apply limit.".into());
    }
    let mut bytes = Vec::new();
    source
        .into_file()
        .take(MAX_APPLY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_APPLY_BYTES {
        return Err("Selected source grew beyond the patch apply limit.".into());
    }
    Ok(bytes)
}

pub(super) fn dat_guidance(
    bytes: &[u8],
    platform: RomFormatContext,
    summaries: &[LibraryDatIdentitySummary],
) -> (Vec<String>, bool) {
    let mut notes = Vec::new();
    let mut refuse = false;
    if summaries.is_empty() {
        notes.push("No cached DAT comparison is available. A title or filename is not patch-source evidence.".into());
    }
    for summary in summaries {
        notes.push(format!(
            "DAT {} · {:?} · {:?} (cached evidence, not a new audit)",
            summary.source.source_name, summary.verification_state, summary.provenance_freshness
        ));
        if summary.provenance_freshness == DatProvenanceFreshness::Stale {
            notes.push(
                "Historical DAT result retained; it is not current source verification.".into(),
            );
            continue;
        }
        if matches!(
            summary.verification_state,
            DatVerificationState::Conflicting { .. }
        ) {
            refuse = true;
            notes.push("Conflicting DAT evidence requires review before patch Apply.".into());
            continue;
        }
        if summary.provenance_freshness != DatProvenanceFreshness::Current || !summary.is_verified()
        {
            continue;
        }
        let mut expected = KnownFileEvidence::new("cached DAT evidence", "selected item");
        let Some(value) = summary.hash_evidence.matched_value.clone() else {
            continue;
        };
        match summary.hash_evidence.matched_algorithm.as_deref() {
            Some("SHA-256") => expected.sha256 = Some(value),
            Some("SHA-1") => expected.sha1 = Some(value),
            Some("MD5") => expected.md5 = Some(value),
            _ => continue,
        }
        let comparison = verify_dat_representation(bytes, platform, &expected);
        if comparison.state == DatRepresentationState::RepresentationEquivalent {
            notes.push("Cached DAT hash matches a different, normalized representation. This explains the layout difference; it does not grant patch applicability or change the DAT verdict.".into());
        } else if comparison.state != DatRepresentationState::RawHashMatch {
            refuse = true;
            notes.push("The cached DAT hash could not be confirmed against this source (mismatch, unsupported evidence or diagnostic budget). Refresh identity and Preview; no automatic normalization is authorized.".into());
        }
    }
    (notes, refuse)
}

pub(super) fn prepare(
    patch: &Path,
    base_path: &Path,
    identity: &GameIdentityReport,
    summaries: &[LibraryDatIdentitySummary],
    output: PathBuf,
    output_root: PathBuf,
) -> Result<StandalonePatchCandidate, String> {
    let inspection = inspect_standalone_patch(patch).map_err(|e| e.to_string())?;
    let bytes = read_base(base_path)?;
    let context = format_context(identity.platform);
    let guidance = preflight_patch_representation(&inspection, &bytes, context);
    let (mut matching, adjustment) = match_patch_source_with_header(
        &inspection,
        Some(&bytes),
        false,
        identity.platform == archivefs_core::game_identity::IdentityPlatform::Snes,
    );
    let (mut notes, mut refuse) = dat_guidance(&bytes, context, summaries);
    let sha = sha256(&bytes);
    for fact in &identity.evidence {
        if fact.kind == archivefs_core::game_identity::IdentityKind::LooseRomSha256
            && fact.status == archivefs_core::game_identity::IdentityStatus::Verified
            && fact
                .value
                .as_deref()
                .is_some_and(|value| value.len() == 64 && !value.eq_ignore_ascii_case(&sha))
        {
            refuse = true;
            notes.push("The selected source differs from reviewed game-identity evidence. Refresh identity and Preview.".into());
        }
    }
    if refuse {
        matching = StandalonePatchMatch { compatibility: PatchCompatibility::Incompatible,
            reason: "Source identity / DAT evidence is contradictory or changed; fresh review is required".into() };
    }
    let mut plan_error = None;
    let plan = if matching.compatibility == PatchCompatibility::Compatible {
        match build_standalone_patch_apply_plan_with_header(
            &inspection,
            base_path,
            output,
            output_root,
            adjustment,
        ) {
            Ok(plan) if plan.reviewed.base_sha256.as_deref() == Some(&sha) => Some(plan),
            Ok(_) => {
                plan_error = Some(
                    "Source changed while Preview was prepared. Choose the patch again.".into(),
                );
                None
            }
            Err(e) => {
                plan_error = Some(e.to_string());
                None
            }
        }
    } else {
        None
    };
    Ok(StandalonePatchCandidate {
        evidence_key: evidence_key(identity, summaries),
        inspection,
        matching,
        adjustment,
        plan,
        plan_error,
        guidance,
        notes,
        source_sha256: sha,
        source_size: bytes.len() as u64,
        source_crc32: crc32(&bytes),
    })
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub(super) fn evidence_key(
    identity: &GameIdentityReport,
    summaries: &[LibraryDatIdentitySummary],
) -> String {
    sha256(&serde_json::to_vec(&(identity, summaries)).expect("identity evidence is serializable"))
}

pub(super) fn show(ui: &mut egui::Ui, guidance: &PatchRepresentationPreflight, notes: &[String]) {
    ui.heading("ROM representation guidance");
    ui.label("Patch CRC32 and size are compatibility checks, not cryptographic proof of a game's identity. Titles and filenames do not authorize Apply.");
    if let Some(diagnostic) = &guidance.representation {
        let level = match diagnostic.equivalence {
            archivefs_core::rom_representation_diagnostics::RepresentationEquivalence::Exact => "Raw checksum match (probable)",
            archivefs_core::rom_representation_diagnostics::RepresentationEquivalence::Equivalent => "Normalized checksum match (advisory only)",
            _ => "Unknown or conflicting representation",
        };
        ui.label(format!("Representation: {:?} · {level}", diagnostic.kind));
        ui.label(&diagnostic.reason);
        ui.small(format!(
            "Observed checksum: {} · Patch expects: {}",
            diagnostic.source_hash, diagnostic.expected_hash
        ));
        let route =
            archivefs_core::conversion_planner::plan_representation_conversion(diagnostic.clone());
        if let Some(operation) = route.recommended_operation {
            ui.label(format!("Required representation operation: {operation}"));
            ui.label("Planning only: the conversion engine has no enabled cartridge route for this suggestion. No conversion or source change will run.");
        }
    } else {
        ui.label("Detailed representation is unknown, unsupported, or beyond the 64 MiB diagnostic budget. No header, offset or byte-order transformation is inferred.");
    }
    ui.label("Patch offsets address the patch's exact input byte layout. Representation-equivalent bytes are not interchangeable patch inputs.");
    for note in notes {
        ui.small(note);
    }
}
