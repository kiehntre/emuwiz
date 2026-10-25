//! Pure review projection for a catalogue record and one selected game.
//!
//! This is presentation/safety data only. It never downloads, resolves a
//! provider URL into a file, creates an apply transaction, or writes a device.

use serde::Serialize;

use crate::game_identity::IdentityStatus;
use crate::mod_catalogue::{
    ModCatalogueCompatibility, ModCatalogueHash, ModCatalogueIdentity, ModCataloguePayload,
    ModCatalogueRecord, ModCatalogueValidationError, ModDestinationIntent,
    assess_catalogue_compatibility,
};
use crate::mod_package::{
    ModCompatibilityState, NativeModIdentityEvidence, SelectedGameForMod,
    project_selected_game_native_identity,
};

const MAX_REVIEW_SUMMARY_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCatalogueReviewStatus {
    Reviewable,
    Blocked,
    ReviewRequired,
    InsufficientEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCatalogueRecordSummary {
    pub title: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub category: crate::mod_catalogue::ModCatalogueCategory,
    pub platform: Option<crate::mod_package::ModCanonicalPlatform>,
    pub description_summary: Option<String>,
    pub provider: String,
    pub provider_record_id: String,
    pub source_page_url: String,
    pub instructions_present: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCataloguePayloadSummary {
    pub payload_id: String,
    pub display_name: Option<String>,
    pub url: Option<String>,
    pub size_bytes: Option<u64>,
    pub supplied_hash: Option<ModCatalogueHash>,
    pub version: Option<String>,
    pub region: Option<String>,
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCatalogueIdentityReview {
    pub declaration: ModCatalogueIdentity,
    pub selected_verified_values: Vec<String>,
    pub outcome: ModCatalogueIdentityOutcome,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCatalogueIdentityOutcome {
    Matches,
    Mismatches,
    Unverified,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCatalogueDestinationReview {
    pub kind: ModCatalogueDestinationKind,
    pub declared_destination: Option<String>,
    pub actual_destination: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCatalogueDestinationKind {
    RelativeGameRoot,
    ReplacementTarget,
    KnownPlatformLocation,
    Manual,
    Unknown,
    Unsafe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCatalogueIntegrityReview {
    pub supplied_hash: Option<ModCatalogueHash>,
    pub payload_bytes_verified: bool,
    pub status: ModCatalogueIntegrityStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCatalogueIntegrityStatus {
    SuppliedNotVerified,
    NoHashSupplied,
    BytesNotAvailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCatalogueRightsStatus {
    Known,
    Unknown,
    RequiresReview,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCatalogueRightsReview {
    pub status: ModCatalogueRightsStatus,
    pub licence: Option<String>,
    pub source_terms_url: Option<String>,
    pub author_or_uploader: Option<String>,
    pub provenance_note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCatalogueReview {
    pub status: ModCatalogueReviewStatus,
    pub record_summary: ModCatalogueRecordSummary,
    pub selected_payload: Option<ModCataloguePayloadSummary>,
    pub provider_provenance: crate::mod_catalogue::ModCatalogueProvenance,
    pub selected_game_identity: NativeModIdentityEvidence,
    pub compatibility: ModCatalogueCompatibility,
    pub identity_evidence: Vec<ModCatalogueIdentityReview>,
    pub destination_review: ModCatalogueDestinationReview,
    pub integrity_review: ModCatalogueIntegrityReview,
    pub rights_review: ModCatalogueRightsReview,
    pub warnings: Vec<String>,
    pub blockers: Vec<String>,
    pub required_next_checks: Vec<String>,
}

pub fn review_catalogue_record(
    record: &ModCatalogueRecord,
    selected_game: &SelectedGameForMod,
    payload_id: Option<&str>,
) -> ModCatalogueReview {
    let record_summary = record_summary(record);
    let selected_game_identity = project_selected_game_native_identity(selected_game);
    let compatibility = assess_catalogue_compatibility(record, selected_game);
    let identity_evidence = identity_reviews(record, selected_game);
    let destination_review = destination_review(&record.destination_intent);
    let selected_payload = select_payload(&record.payloads, payload_id);
    let integrity_review = integrity_review(selected_payload.as_ref());
    let rights_review = rights_review(record);
    let mut warnings = Vec::new();
    let mut blockers = Vec::new();
    let mut required_next_checks = Vec::new();
    let payload_selection_required = record.payloads.len() > 1 && payload_id.is_none();

    if let Err(errors) = record.validate() {
        for error in errors {
            blockers.push(format_validation_error(error));
        }
    }
    if payload_selection_required {
        required_next_checks.push("user selects one payload/version".into());
    } else if selected_payload.is_none() {
        blockers.push("selected payload does not exist in the catalogue record".into());
        required_next_checks.push("user selects an available payload".into());
    }
    if compatibility.state == ModCompatibilityState::Incompatible {
        blockers.extend(compatibility.reasons.iter().cloned());
    } else if compatibility.state == ModCompatibilityState::Unknown {
        warnings.extend(compatibility.reasons.iter().cloned());
        required_next_checks.push("obtain stronger native identity evidence".into());
    }
    if destination_review.kind == ModCatalogueDestinationKind::Unsafe {
        blockers.push("provider destination intent is unsafe and cannot be used".into());
    } else {
        required_next_checks.push("resolve and review a concrete destination".into());
    }
    if let Some(payload) = selected_payload.as_ref() {
        required_next_checks
            .push("download the selected payload through a bounded downloader".into());
        if payload.supplied_hash.is_some() {
            required_next_checks.push("verify downloaded bytes against the supplied hash".into());
        } else {
            warnings.push(
                "payload has no supplied hash; downloaded bytes will need independent verification"
                    .into(),
            );
        }
        required_next_checks.push("safely inspect the downloaded archive or file".into());
    }
    required_next_checks.push("revalidate selected-game identity before any future apply".into());
    required_next_checks.push("construct a separate reviewed backup/transaction plan".into());
    warnings.extend(instruction_warnings(record.instructions.as_deref()));
    if rights_review.status != ModCatalogueRightsStatus::Known {
        warnings.push("licence/redistribution rights are not fully established".into());
    }
    if !selected_game_identity.warnings.is_empty() {
        warnings.extend(selected_game_identity.warnings.iter().cloned());
    }
    if selected_game_identity.status == IdentityStatus::Ambiguous {
        blockers.push("selected-game native identity evidence is conflicting".into());
    }
    sort_unique(&mut warnings);
    sort_unique(&mut blockers);
    sort_unique(&mut required_next_checks);

    let status = if !blockers.is_empty() {
        ModCatalogueReviewStatus::Blocked
    } else if payload_selection_required
        || selected_payload.is_none()
        || compatibility.state == ModCompatibilityState::Unknown
    {
        ModCatalogueReviewStatus::ReviewRequired
    } else if compatibility.state == ModCompatibilityState::Compatible {
        ModCatalogueReviewStatus::Reviewable
    } else {
        ModCatalogueReviewStatus::InsufficientEvidence
    };
    ModCatalogueReview {
        status,
        record_summary,
        selected_payload,
        provider_provenance: record.provenance.clone(),
        selected_game_identity,
        compatibility,
        identity_evidence,
        destination_review,
        integrity_review,
        rights_review,
        warnings,
        blockers,
        required_next_checks,
    }
}

fn record_summary(record: &ModCatalogueRecord) -> ModCatalogueRecordSummary {
    ModCatalogueRecordSummary {
        title: bounded_summary(&record.display_title),
        author: record.author.as_deref().map(bounded_summary),
        version: record.version.as_deref().map(bounded_summary),
        category: record.category.clone(),
        platform: record.platform,
        description_summary: record.description.as_deref().map(bounded_summary),
        provider: bounded_summary(&record.provider.name),
        provider_record_id: bounded_summary(&record.provider.record_id),
        source_page_url: record.provider.source_page_url.clone(),
        instructions_present: record.instructions.is_some(),
    }
}

fn select_payload(
    payloads: &[ModCataloguePayload],
    payload_id: Option<&str>,
) -> Option<ModCataloguePayloadSummary> {
    let payload = match payload_id {
        Some(id) => payloads.iter().find(|payload| payload.payload_id == id),
        None if payloads.len() == 1 => payloads.first(),
        None => None,
    }?;
    Some(ModCataloguePayloadSummary {
        payload_id: payload.payload_id.clone(),
        display_name: payload.display_name.clone(),
        url: payload.url.clone(),
        size_bytes: payload.size_bytes,
        supplied_hash: payload.hash.clone(),
        version: payload.version.clone(),
        region: payload.region.clone(),
        notes: payload.notes.clone(),
    })
}

fn identity_reviews(
    record: &ModCatalogueRecord,
    selected_game: &SelectedGameForMod,
) -> Vec<ModCatalogueIdentityReview> {
    let mut reviews: Vec<_> = record
        .declared_identity
        .iter()
        .map(|declaration| {
            let mut values: Vec<_> = selected_game
                .identity
                .evidence
                .iter()
                .filter(|item| {
                    item.kind == declaration.kind.identity_kind()
                        && item.status == IdentityStatus::Verified
                })
                .filter_map(|item| item.value.clone())
                .collect();
            values.sort();
            values.dedup();
            let outcome = if values.iter().any(|value| value == &declaration.value) {
                ModCatalogueIdentityOutcome::Matches
            } else if values.is_empty() {
                ModCatalogueIdentityOutcome::Unverified
            } else {
                ModCatalogueIdentityOutcome::Mismatches
            };
            ModCatalogueIdentityReview {
                declaration: declaration.clone(),
                selected_verified_values: values,
                outcome,
            }
        })
        .collect();
    reviews.sort_by(|left, right| {
        format!("{:?}:{}", left.declaration.kind, left.declaration.value).cmp(&format!(
            "{:?}:{}",
            right.declaration.kind, right.declaration.value
        ))
    });
    reviews
}

fn destination_review(intent: &ModDestinationIntent) -> ModCatalogueDestinationReview {
    let (kind, declared_destination) = match intent {
        ModDestinationIntent::GameRootRelative { path } => (
            ModCatalogueDestinationKind::RelativeGameRoot,
            Some(path.to_string_lossy().into_owned()),
        ),
        ModDestinationIntent::ReplacementTarget { path } => (
            ModCatalogueDestinationKind::ReplacementTarget,
            Some(path.to_string_lossy().into_owned()),
        ),
        ModDestinationIntent::KnownPlatformLocation { label } => (
            ModCatalogueDestinationKind::KnownPlatformLocation,
            Some(label.clone()),
        ),
        ModDestinationIntent::Manual => (ModCatalogueDestinationKind::Manual, None),
        ModDestinationIntent::Unknown => (ModCatalogueDestinationKind::Unknown, None),
    };
    let unsafe_path = matches!(
        intent,
        ModDestinationIntent::GameRootRelative { path }
            | ModDestinationIntent::ReplacementTarget { path }
            if path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
    );
    ModCatalogueDestinationReview {
        kind: if unsafe_path {
            ModCatalogueDestinationKind::Unsafe
        } else {
            kind
        },
        declared_destination,
        actual_destination: None,
    }
}

fn integrity_review(payload: Option<&ModCataloguePayloadSummary>) -> ModCatalogueIntegrityReview {
    match payload.and_then(|payload| payload.supplied_hash.clone()) {
        Some(hash) => ModCatalogueIntegrityReview {
            supplied_hash: Some(hash),
            payload_bytes_verified: false,
            status: ModCatalogueIntegrityStatus::SuppliedNotVerified,
        },
        None => ModCatalogueIntegrityReview {
            supplied_hash: None,
            payload_bytes_verified: false,
            status: if payload.is_some() {
                ModCatalogueIntegrityStatus::NoHashSupplied
            } else {
                ModCatalogueIntegrityStatus::BytesNotAvailable
            },
        },
    }
}

fn rights_review(record: &ModCatalogueRecord) -> ModCatalogueRightsReview {
    let status = match (
        record.provenance.licence.is_some(),
        record.provenance.source_terms_url.is_some(),
    ) {
        (true, _) => ModCatalogueRightsStatus::Known,
        (false, true) => ModCatalogueRightsStatus::RequiresReview,
        (false, false) => ModCatalogueRightsStatus::Unknown,
    };
    ModCatalogueRightsReview {
        status,
        licence: record.provenance.licence.clone(),
        source_terms_url: record.provenance.source_terms_url.clone(),
        author_or_uploader: record.provenance.author_or_uploader.clone(),
        provenance_note: record.provenance.note.clone(),
    }
}

fn instruction_warnings(instructions: Option<&str>) -> Vec<String> {
    let Some(text) = instructions else {
        return Vec::new();
    };
    let lower = text.to_ascii_lowercase();
    let mut warnings = Vec::new();
    if lower.contains("execute")
        || lower.contains(".exe")
        || lower.contains(".bat")
        || lower.contains(".sh")
    {
        warnings.push("instructions mention executing a program or script".into());
    }
    if lower.contains("disable") && (lower.contains("protection") || lower.contains("security")) {
        warnings.push("instructions mention disabling protections".into());
    }
    if lower.contains("/dev_hdd0/") || lower.contains("absolute console path") {
        warnings.push("instructions mention an absolute console destination".into());
    }
    if lower.contains("overwrite system") {
        warnings.push("instructions mention overwriting system files".into());
    }
    warnings
}

fn format_validation_error(error: ModCatalogueValidationError) -> String {
    error.to_string()
}

fn bounded_summary(value: &str) -> String {
    if value.len() <= MAX_REVIEW_SUMMARY_BYTES {
        value.to_string()
    } else {
        let mut end = MAX_REVIEW_SUMMARY_BYTES;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &value[..end])
    }
}

fn sort_unique(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_identity::{
        GameIdentityReport, IdentityConfidence, IdentityEvidence, IdentityImageFormat,
        IdentityKind, IdentityPlatform, IdentityProvenance,
    };
    use crate::mod_catalogue::{
        ModCatalogueCategory, ModCatalogueHash, ModCatalogueHashAlgorithm, ModCatalogueProvenance,
        ModCatalogueProvider,
    };
    use crate::mod_package::{ModCanonicalPlatform, ModIdentityKind};
    use std::path::PathBuf;

    fn selected(
        platform: IdentityPlatform,
        evidence: Vec<(IdentityKind, &str)>,
    ) -> SelectedGameForMod {
        SelectedGameForMod {
            game_root: PathBuf::from("/tmp/review-game"),
            identity: GameIdentityReport {
                archive_path: PathBuf::from("/tmp/review-game/default.xex"),
                platform,
                format: IdentityImageFormat::Xex,
                evidence: evidence
                    .into_iter()
                    .map(|(kind, value)| IdentityEvidence {
                        kind,
                        status: IdentityStatus::Verified,
                        value: Some(value.into()),
                        confidence: IdentityConfidence::ExactBytes,
                        provenance: IdentityProvenance {
                            archive_path: PathBuf::from("/tmp/review-fixture"),
                            member_path: None,
                            member_index: None,
                            method: "synthetic".into(),
                        },
                        diagnostic: String::new(),
                    })
                    .collect(),
                warnings: Vec::new(),
                bytes_read: 0,
                archive_members_inspected: 0,
                metadata_paths_inspected: 0,
                nested_container_depth: 0,
                complete: true,
            },
        }
    }

    fn record(
        identities: Vec<(ModIdentityKind, &str)>,
        payloads: Vec<ModCataloguePayload>,
    ) -> ModCatalogueRecord {
        ModCatalogueRecord {
            provider: ModCatalogueProvider {
                name: "fixture-provider".into(),
                record_id: "record-1".into(),
                source_page_url: "https://example.invalid/record-1".into(),
                schema_version: None,
                imported_at: None,
                snapshot_sha256: None,
            },
            display_title: "Fixture mod".into(),
            author: Some("Fixture author".into()),
            version: Some("1.0".into()),
            description: Some("Read-only review fixture".into()),
            title_hint: Some("Fixture game".into()),
            platform: Some(ModCanonicalPlatform::PlayStation3),
            category: ModCatalogueCategory::GameMod,
            payloads,
            declared_identity: identities
                .into_iter()
                .map(|(kind, value)| ModCatalogueIdentity {
                    kind,
                    value: value.into(),
                })
                .collect(),
            declared_region: None,
            declared_revision: None,
            destination_intent: ModDestinationIntent::GameRootRelative {
                path: PathBuf::from("USRDIR/mod.bin"),
            },
            instructions: Some("Do not execute an EXE; copy to a reviewed target.".into()),
            provenance: ModCatalogueProvenance {
                source_terms_url: None,
                licence: None,
                author_or_uploader: Some("fixture-uploader".into()),
                note: Some("synthetic".into()),
            },
            rom_hack: None,
        }
    }

    fn payload(id: &str) -> ModCataloguePayload {
        ModCataloguePayload {
            payload_id: id.into(),
            display_name: Some(id.into()),
            url: Some(format!("https://example.invalid/{id}.zip")),
            size_bytes: Some(10),
            hash: None,
            version: Some("1".into()),
            region: None,
            notes: None,
            archive_hint: Some("zip".into()),
        }
    }

    #[test]
    fn matching_ps3_review_is_reviewable_but_download_and_destination_remain_pending() {
        let review = review_catalogue_record(
            &record(
                vec![(ModIdentityKind::Ps3TitleId, "BLUS30000")],
                vec![payload("usa")],
            ),
            &selected(
                IdentityPlatform::PlayStation3,
                vec![(IdentityKind::Ps3TitleId, "BLUS30000")],
            ),
            None,
        );
        assert_eq!(review.status, ModCatalogueReviewStatus::Reviewable);
        assert_eq!(
            review.compatibility.state,
            ModCompatibilityState::Compatible
        );
        assert_eq!(
            review.integrity_review.status,
            ModCatalogueIntegrityStatus::NoHashSupplied
        );
        assert!(review.destination_review.actual_destination.is_none());
        assert!(
            review
                .required_next_checks
                .iter()
                .any(|check| check.contains("download"))
        );
        assert!(
            review
                .warnings
                .iter()
                .any(|warning| warning.contains("executing"))
        );
    }

    #[test]
    fn xbox_media_mismatch_is_blocked() {
        let mut record = record(
            vec![
                (ModIdentityKind::XexTitleId, "584109D2"),
                (ModIdentityKind::XexMediaId, "12345678"),
            ],
            vec![payload("xbox")],
        );
        record.platform = Some(ModCanonicalPlatform::Xbox360);
        let review = review_catalogue_record(
            &record,
            &selected(
                IdentityPlatform::Xbox360,
                vec![
                    (IdentityKind::XexTitleId, "584109D2"),
                    (IdentityKind::XexMediaId, "BAD00000"),
                ],
            ),
            Some("xbox"),
        );
        assert_eq!(review.status, ModCatalogueReviewStatus::Blocked);
        assert!(
            review
                .identity_evidence
                .iter()
                .any(|item| item.outcome == ModCatalogueIdentityOutcome::Mismatches)
        );
    }

    #[test]
    fn multiple_payloads_require_explicit_selection() {
        let review = review_catalogue_record(
            &record(
                vec![(ModIdentityKind::Ps3TitleId, "BLUS30000")],
                vec![payload("eu"), payload("usa")],
            ),
            &selected(
                IdentityPlatform::PlayStation3,
                vec![(IdentityKind::Ps3TitleId, "BLUS30000")],
            ),
            None,
        );
        assert_eq!(review.status, ModCatalogueReviewStatus::ReviewRequired);
        assert!(review.selected_payload.is_none());
        assert!(
            review
                .required_next_checks
                .iter()
                .any(|check| check.contains("selects one payload"))
        );
    }

    #[test]
    fn supplied_hash_is_not_verified_and_unsafe_destination_is_blocked() {
        let mut record = record(
            vec![(ModIdentityKind::Ps3TitleId, "BLUS30000")],
            vec![payload("ps3")],
        );
        record.payloads[0].hash = Some(ModCatalogueHash {
            algorithm: ModCatalogueHashAlgorithm::Sha256,
            value: "a".repeat(64),
        });
        record.destination_intent = ModDestinationIntent::GameRootRelative {
            path: PathBuf::from("../escape"),
        };
        let review = review_catalogue_record(
            &record,
            &selected(
                IdentityPlatform::PlayStation3,
                vec![(IdentityKind::Ps3TitleId, "BLUS30000")],
            ),
            Some("ps3"),
        );
        assert_eq!(review.status, ModCatalogueReviewStatus::Blocked);
        assert!(!review.integrity_review.payload_bytes_verified);
        assert_eq!(
            review.integrity_review.status,
            ModCatalogueIntegrityStatus::SuppliedNotVerified
        );
        assert_eq!(
            review.destination_review.kind,
            ModCatalogueDestinationKind::Unsafe
        );
    }

    #[test]
    fn title_only_review_is_unknown_and_next_checks_are_deterministic() {
        let record = record(Vec::new(), vec![payload("title-only")]);
        let first = review_catalogue_record(
            &record,
            &selected(IdentityPlatform::PlayStation3, vec![]),
            Some("title-only"),
        );
        let second = review_catalogue_record(
            &record,
            &selected(IdentityPlatform::PlayStation3, vec![]),
            Some("title-only"),
        );
        assert_eq!(first.status, ModCatalogueReviewStatus::ReviewRequired);
        assert_eq!(first.compatibility.state, ModCompatibilityState::Unknown);
        assert_eq!(first.required_next_checks, second.required_next_checks);
        assert_eq!(first.warnings, second.warnings);
    }
}
