//! Provider-neutral, metadata-only mod catalogue records.
//!
//! This module deliberately has no network, archive, filesystem-write, or
//! install behavior. URLs, hashes, and provider declarations are descriptive
//! metadata until a later download/verification phase proves them.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use url::Url;

use crate::game_identity::{GameIdentityReport, IdentityKind, IdentityStatus};
use crate::mod_package::{
    ModCanonicalPlatform, ModCompatibilityState, ModIdentityKind, SelectedGameForMod,
};

pub const MAX_CATALOGUE_TEXT_BYTES: usize = 4096;
pub const MAX_CATALOGUE_URL_BYTES: usize = 8192;
pub const MAX_CATALOGUE_PAYLOADS: usize = 64;
pub const MAX_CATALOGUE_IDENTITIES: usize = 32;
pub const MAX_CATALOGUE_INSTRUCTIONS_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModCatalogueProvider {
    pub name: String,
    pub record_id: String,
    pub source_page_url: String,
    pub schema_version: Option<String>,
    pub imported_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModCatalogueCategory {
    GameMod,
    TextureResource,
    Save,
    CheatTrainer,
    PluginTool,
    Package,
    Homebrew,
    Other(String),
    Unknown,
}

impl ModCatalogueCategory {
    pub fn from_provider_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "game_mod" | "game-mod" | "mod" => Self::GameMod,
            "texture" | "texture_resource" | "texture-resource" => Self::TextureResource,
            "save" | "savedata" => Self::Save,
            "cheat" | "trainer" | "cheat_trainer" => Self::CheatTrainer,
            "plugin" | "tool" | "plugin_tool" => Self::PluginTool,
            "package" | "archive" => Self::Package,
            "homebrew" => Self::Homebrew,
            "" => Self::Unknown,
            other => Self::Other(other.to_string()),
        }
    }
}

impl Serialize for ModCatalogueCategory {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = match self {
            Self::GameMod => "game_mod",
            Self::TextureResource => "texture_resource",
            Self::Save => "save",
            Self::CheatTrainer => "cheat_trainer",
            Self::PluginTool => "plugin_tool",
            Self::Package => "package",
            Self::Homebrew => "homebrew",
            Self::Unknown => "unknown",
            Self::Other(value) => value,
        };
        serializer.serialize_str(value)
    }
}

impl<'de> Deserialize<'de> for ModCatalogueCategory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct CategoryVisitor;
        impl<'de> Visitor<'de> for CategoryVisitor {
            type Value = ModCatalogueCategory;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a catalogue category string")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(ModCatalogueCategory::from_provider_value(value))
            }
        }
        deserializer.deserialize_str(CategoryVisitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCatalogueHashAlgorithm {
    Sha256,
    Sha1,
    Md5,
    Unknown(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModCatalogueHash {
    pub algorithm: ModCatalogueHashAlgorithm,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModCataloguePayload {
    pub payload_id: String,
    pub display_name: Option<String>,
    pub url: Option<String>,
    pub size_bytes: Option<u64>,
    pub hash: Option<ModCatalogueHash>,
    pub version: Option<String>,
    pub region: Option<String>,
    pub notes: Option<String>,
    pub archive_hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModCatalogueIdentity {
    pub kind: ModIdentityKind,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModDestinationIntent {
    GameRootRelative { path: PathBuf },
    ReplacementTarget { path: PathBuf },
    KnownPlatformLocation { label: String },
    Manual,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModCatalogueProvenance {
    pub source_terms_url: Option<String>,
    pub licence: Option<String>,
    pub author_or_uploader: Option<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModCatalogueRecord {
    pub provider: ModCatalogueProvider,
    pub display_title: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub title_hint: Option<String>,
    pub platform: Option<ModCanonicalPlatform>,
    pub category: ModCatalogueCategory,
    pub payloads: Vec<ModCataloguePayload>,
    pub declared_identity: Vec<ModCatalogueIdentity>,
    pub declared_region: Option<String>,
    pub declared_revision: Option<String>,
    pub destination_intent: ModDestinationIntent,
    pub instructions: Option<String>,
    pub provenance: ModCatalogueProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModCatalogueValidationError {
    pub field: String,
    pub detail: String,
}

impl fmt::Display for ModCatalogueValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.detail)
    }
}

impl ModCatalogueRecord {
    pub fn validate(&self) -> Result<(), Vec<ModCatalogueValidationError>> {
        let mut errors = Vec::new();
        bounded_required(&mut errors, "provider.name", &self.provider.name);
        bounded_required(&mut errors, "provider.record_id", &self.provider.record_id);
        bounded_text(&mut errors, "display_title", &self.display_title);
        bounded_optional(&mut errors, "author", self.author.as_deref());
        bounded_optional(&mut errors, "version", self.version.as_deref());
        bounded_optional(&mut errors, "description", self.description.as_deref());
        bounded_optional(&mut errors, "title_hint", self.title_hint.as_deref());
        bounded_optional(
            &mut errors,
            "declared_region",
            self.declared_region.as_deref(),
        );
        bounded_optional(
            &mut errors,
            "declared_revision",
            self.declared_revision.as_deref(),
        );
        if let Some(instructions) = self.instructions.as_deref()
            && instructions.len() > MAX_CATALOGUE_INSTRUCTIONS_BYTES
        {
            errors.push(error("instructions", "text exceeds the catalogue bound"));
        }
        validate_url(
            &mut errors,
            "provider.source_page_url",
            &self.provider.source_page_url,
        );
        if let Some(url) = self.provenance.source_terms_url.as_deref() {
            validate_url(&mut errors, "provenance.source_terms_url", url);
        }
        if self.payloads.len() > MAX_CATALOGUE_PAYLOADS {
            errors.push(error(
                "payloads",
                "payload count exceeds the catalogue bound",
            ));
        }
        if self.declared_identity.len() > MAX_CATALOGUE_IDENTITIES {
            errors.push(error(
                "declared_identity",
                "identity count exceeds the catalogue bound",
            ));
        }
        let mut payload_ids = BTreeSet::new();
        let mut hashes = BTreeSet::new();
        for payload in &self.payloads {
            bounded_required(&mut errors, "payload.payload_id", &payload.payload_id);
            if !payload_ids.insert(payload.payload_id.clone()) {
                errors.push(error("payload.payload_id", "duplicate payload ID"));
            }
            bounded_optional(
                &mut errors,
                "payload.display_name",
                payload.display_name.as_deref(),
            );
            bounded_optional(&mut errors, "payload.version", payload.version.as_deref());
            bounded_optional(&mut errors, "payload.region", payload.region.as_deref());
            bounded_optional(&mut errors, "payload.notes", payload.notes.as_deref());
            bounded_optional(
                &mut errors,
                "payload.archive_hint",
                payload.archive_hint.as_deref(),
            );
            if let Some(url) = payload.url.as_deref() {
                validate_url(&mut errors, "payload.url", url);
            }
            if let Some(hash) = &payload.hash {
                if !valid_hash_value(&hash.algorithm, &hash.value) {
                    errors.push(error(
                        "payload.hash",
                        "unsupported algorithm or invalid digest",
                    ));
                }
                if !hashes.insert(format!(
                    "{:?}:{}",
                    hash.algorithm,
                    hash.value.to_ascii_lowercase()
                )) {
                    errors.push(error("payload.hash", "duplicate supplied hash"));
                }
            }
        }
        for identity in &self.declared_identity {
            if identity.value.trim().is_empty() || identity.value.len() > MAX_CATALOGUE_TEXT_BYTES {
                errors.push(error(
                    "declared_identity.value",
                    "identity value is empty or too long",
                ));
            }
            if matches!(identity.kind, ModIdentityKind::Ps3TitleId)
                && !valid_ps3_title_id(&identity.value)
            {
                errors.push(error(
                    "declared_identity.value",
                    "invalid PS3 Title ID syntax",
                ));
            }
            if matches!(
                identity.kind,
                ModIdentityKind::XexTitleId | ModIdentityKind::XexMediaId
            ) && !valid_hex_identity(&identity.value, 8)
            {
                errors.push(error(
                    "declared_identity.value",
                    "invalid Xbox identity syntax",
                ));
            }
        }
        validate_destination(&mut errors, &self.destination_intent);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn canonicalized(mut self) -> Self {
        self.payloads
            .sort_by(|left, right| left.payload_id.cmp(&right.payload_id));
        self.declared_identity.sort_by(|left, right| {
            format!("{:?}:{}", left.kind, left.value)
                .cmp(&format!("{:?}:{}", right.kind, right.value))
        });
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModCatalogueMatchStrength {
    Strong,
    Weak,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModCatalogueCompatibility {
    pub state: ModCompatibilityState,
    pub strength: ModCatalogueMatchStrength,
    pub matching_identity: Option<ModCatalogueIdentity>,
    pub reasons: Vec<String>,
    pub warnings: Vec<String>,
}

/// Compares catalogue declarations with native selected-game evidence. Every
/// provider declaration remains candidate metadata; only the selected game's
/// existing verified evidence can make a positive match.
pub fn assess_catalogue_compatibility(
    record: &ModCatalogueRecord,
    selected_game: &SelectedGameForMod,
) -> ModCatalogueCompatibility {
    let mut result = ModCatalogueCompatibility {
        state: ModCompatibilityState::Unknown,
        strength: ModCatalogueMatchStrength::Unknown,
        matching_identity: None,
        reasons: Vec::new(),
        warnings: Vec::new(),
    };
    if let Some(platform) = record.platform
        && platform.identity_platform() != selected_game.identity.platform
    {
        result.state = ModCompatibilityState::Incompatible;
        result
            .reasons
            .push("catalogue platform does not match selected game".into());
        return result;
    }
    let native: Vec<_> = record
        .declared_identity
        .iter()
        .filter(|identity| is_native_identity(identity.kind))
        .collect();
    if native.is_empty() {
        result.strength = if record.title_hint.is_some() {
            ModCatalogueMatchStrength::Weak
        } else {
            ModCatalogueMatchStrength::Unknown
        };
        result
            .reasons
            .push("catalogue has no native game identity; title hints remain unverified".into());
        return result;
    }
    let requires_media = native
        .iter()
        .any(|item| item.kind == ModIdentityKind::XexMediaId);
    for declaration in &native {
        let evidence = verified_values(&selected_game.identity, declaration.kind.identity_kind());
        if evidence.iter().any(|value| value == &declaration.value) {
            result.matching_identity = Some((*declaration).clone());
            if declaration.kind == ModIdentityKind::XexMediaId || !requires_media {
                result.state = ModCompatibilityState::Compatible;
                result.strength = if requires_media {
                    ModCatalogueMatchStrength::Strong
                } else {
                    ModCatalogueMatchStrength::Strong
                };
                result.reasons.push(format!(
                    "verified native {} matches catalogue declaration",
                    declaration.kind.identity_kind()
                ));
            }
        } else if !evidence.is_empty() {
            result.state = ModCompatibilityState::Incompatible;
            result.reasons.push(format!(
                "verified native {} conflicts with catalogue declaration",
                declaration.kind.identity_kind()
            ));
            return result;
        }
    }
    if result.state == ModCompatibilityState::Compatible {
        return result;
    }
    if requires_media
        && native
            .iter()
            .any(|item| item.kind == ModIdentityKind::XexTitleId)
    {
        if result.matching_identity.is_some() {
            result.state = ModCompatibilityState::Compatible;
            result.strength = ModCatalogueMatchStrength::Weak;
            result.warnings.push(
                "Xbox Media ID evidence is incomplete; Title ID provides only weaker compatibility"
                    .into(),
            );
            result
                .reasons
                .push("native Media ID was not verified".into());
            return result;
        }
    }
    result
        .reasons
        .push("required native identity is not verified for the selected game".into());
    result
}

fn is_native_identity(kind: ModIdentityKind) -> bool {
    matches!(
        kind,
        ModIdentityKind::Ps3TitleId | ModIdentityKind::XexTitleId | ModIdentityKind::XexMediaId
    )
}

fn verified_values(report: &GameIdentityReport, kind: IdentityKind) -> BTreeSet<String> {
    report
        .evidence
        .iter()
        .filter(|item| item.kind == kind && item.status == IdentityStatus::Verified)
        .filter_map(|item| item.value.clone())
        .collect()
}

fn error(field: &str, detail: &str) -> ModCatalogueValidationError {
    ModCatalogueValidationError {
        field: field.into(),
        detail: detail.into(),
    }
}

fn bounded_required(errors: &mut Vec<ModCatalogueValidationError>, field: &str, value: &str) {
    if value.trim().is_empty() || value.len() > MAX_CATALOGUE_TEXT_BYTES || value.contains('\0') {
        errors.push(error(
            field,
            "value is empty, contains NUL, or exceeds the text bound",
        ));
    }
}

fn bounded_text(errors: &mut Vec<ModCatalogueValidationError>, field: &str, value: &str) {
    if value.len() > MAX_CATALOGUE_TEXT_BYTES || value.contains('\0') {
        errors.push(error(field, "text contains NUL or exceeds the text bound"));
    }
}

fn bounded_optional(
    errors: &mut Vec<ModCatalogueValidationError>,
    field: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        bounded_text(errors, field, value);
    }
}

fn validate_url(errors: &mut Vec<ModCatalogueValidationError>, field: &str, value: &str) {
    if value.len() > MAX_CATALOGUE_URL_BYTES
        || Url::parse(value).is_err()
        || !value.starts_with("https://") && !value.starts_with("http://")
    {
        errors.push(error(
            field,
            "URL is malformed or uses an unsupported scheme",
        ));
    }
}

fn valid_hash_value(algorithm: &ModCatalogueHashAlgorithm, value: &str) -> bool {
    let expected = match algorithm {
        ModCatalogueHashAlgorithm::Sha256 => 64,
        ModCatalogueHashAlgorithm::Sha1 => 40,
        ModCatalogueHashAlgorithm::Md5 => 32,
        ModCatalogueHashAlgorithm::Unknown(_) => return false,
    };
    value.len() == expected && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_ps3_title_id(value: &str) -> bool {
    value.len() == 9
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value[1..].bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn valid_hex_identity(value: &str, width: usize) -> bool {
    value.len() == width && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_destination(
    errors: &mut Vec<ModCatalogueValidationError>,
    intent: &ModDestinationIntent,
) {
    let path = match intent {
        ModDestinationIntent::GameRootRelative { path }
        | ModDestinationIntent::ReplacementTarget { path } => Some(path.as_path()),
        ModDestinationIntent::KnownPlatformLocation { label } => {
            bounded_required(errors, "destination_intent.label", label);
            None
        }
        ModDestinationIntent::Manual | ModDestinationIntent::Unknown => None,
    };
    if let Some(path) = path
        && !safe_relative_path(path)
    {
        errors.push(error(
            "destination_intent.path",
            "path must be relative and confined",
        ));
    }
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_identity::{
        IdentityConfidence, IdentityEvidence, IdentityImageFormat, IdentityPlatform,
        IdentityProvenance,
    };

    fn selected(
        platform: IdentityPlatform,
        evidence: Vec<(IdentityKind, &str)>,
    ) -> SelectedGameForMod {
        SelectedGameForMod {
            game_root: PathBuf::from("/tmp/synthetic-game"),
            identity: GameIdentityReport {
                archive_path: PathBuf::from("/tmp/synthetic-game/default.xex"),
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
                            archive_path: PathBuf::from("/tmp/synthetic"),
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
        platform: ModCanonicalPlatform,
        identities: Vec<(ModIdentityKind, &str)>,
    ) -> ModCatalogueRecord {
        ModCatalogueRecord {
            provider: ModCatalogueProvider {
                name: "synthetic".into(),
                record_id: "record-1".into(),
                source_page_url: "https://example.invalid/mod/record-1".into(),
                schema_version: None,
                imported_at: None,
            },
            display_title: "Synthetic mod".into(),
            author: None,
            version: Some("1".into()),
            description: None,
            title_hint: Some("Synthetic game".into()),
            platform: Some(platform),
            category: ModCatalogueCategory::GameMod,
            payloads: vec![ModCataloguePayload {
                payload_id: "payload-a".into(),
                display_name: Some("payload".into()),
                url: Some("https://example.invalid/payload.zip".into()),
                size_bytes: None,
                hash: None,
                version: None,
                region: None,
                notes: None,
                archive_hint: Some("zip".into()),
            }],
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
                path: PathBuf::from("mods/file.bin"),
            },
            instructions: Some("Copy according to the later reviewed plan.".into()),
            provenance: ModCatalogueProvenance {
                source_terms_url: None,
                licence: None,
                author_or_uploader: None,
                note: Some("metadata fixture".into()),
            },
        }
    }

    #[test]
    fn ps3_native_title_match_and_mismatch_are_projected() {
        let record = record(
            ModCanonicalPlatform::PlayStation3,
            vec![(ModIdentityKind::Ps3TitleId, "BLUS30000")],
        );
        assert_eq!(
            assess_catalogue_compatibility(
                &record,
                &selected(
                    IdentityPlatform::PlayStation3,
                    vec![(IdentityKind::Ps3TitleId, "BLUS30000")]
                )
            )
            .state,
            ModCompatibilityState::Compatible
        );
        assert_eq!(
            assess_catalogue_compatibility(
                &record,
                &selected(
                    IdentityPlatform::PlayStation3,
                    vec![(IdentityKind::Ps3TitleId, "BLUS30001")]
                )
            )
            .state,
            ModCompatibilityState::Incompatible
        );
    }

    #[test]
    fn xbox_title_and_media_are_strong_but_media_mismatch_is_incompatible() {
        let record = record(
            ModCanonicalPlatform::Xbox360,
            vec![
                (ModIdentityKind::XexTitleId, "584109D2"),
                (ModIdentityKind::XexMediaId, "12345678"),
            ],
        );
        let strong = assess_catalogue_compatibility(
            &record,
            &selected(
                IdentityPlatform::Xbox360,
                vec![
                    (IdentityKind::XexTitleId, "584109D2"),
                    (IdentityKind::XexMediaId, "12345678"),
                ],
            ),
        );
        assert_eq!(strong.state, ModCompatibilityState::Compatible);
        assert_eq!(strong.strength, ModCatalogueMatchStrength::Strong);
        let mismatch = assess_catalogue_compatibility(
            &record,
            &selected(
                IdentityPlatform::Xbox360,
                vec![
                    (IdentityKind::XexTitleId, "584109D2"),
                    (IdentityKind::XexMediaId, "BAD00000"),
                ],
            ),
        );
        assert_eq!(mismatch.state, ModCompatibilityState::Incompatible);
    }

    #[test]
    fn title_only_is_weak_and_missing_native_evidence_stays_unknown() {
        let title_only = record(ModCanonicalPlatform::PlayStation3, Vec::new());
        let result = assess_catalogue_compatibility(
            &title_only,
            &selected(IdentityPlatform::PlayStation3, vec![]),
        );
        assert_eq!(result.state, ModCompatibilityState::Unknown);
        assert_eq!(result.strength, ModCatalogueMatchStrength::Weak);
        let declaration = record(
            ModCanonicalPlatform::PlayStation3,
            vec![(ModIdentityKind::Ps3TitleId, "BLUS30000")],
        );
        let missing = assess_catalogue_compatibility(
            &declaration,
            &selected(IdentityPlatform::PlayStation3, vec![]),
        );
        assert_eq!(missing.state, ModCompatibilityState::Unknown);
    }

    #[test]
    fn validation_rejects_unsafe_urls_paths_duplicates_and_unknown_hashes() {
        let mut record = record(
            ModCanonicalPlatform::PlayStation3,
            vec![(ModIdentityKind::Ps3TitleId, "BLUS30000")],
        );
        record.provider.source_page_url = "file:///private".into();
        record.destination_intent = ModDestinationIntent::GameRootRelative {
            path: PathBuf::from("../escape"),
        };
        record.payloads.push(record.payloads[0].clone());
        record.payloads[0].hash = Some(ModCatalogueHash {
            algorithm: ModCatalogueHashAlgorithm::Unknown("sha999".into()),
            value: "abc".into(),
        });
        let errors = record.validate().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.field == "provider.source_page_url")
        );
        assert!(
            errors
                .iter()
                .any(|error| error.field == "destination_intent.path")
        );
        assert!(
            errors
                .iter()
                .any(|error| error.detail == "duplicate payload ID")
        );
        assert!(
            errors
                .iter()
                .any(|error| error.detail == "unsupported algorithm or invalid digest")
        );
    }

    #[test]
    fn supplied_hash_is_metadata_and_canonical_serialization_is_deterministic() {
        let mut record = record(
            ModCanonicalPlatform::PlayStation3,
            vec![(ModIdentityKind::Ps3TitleId, "BLUS30000")],
        );
        record.payloads.push(ModCataloguePayload {
            payload_id: "payload-b".into(),
            display_name: None,
            url: None,
            size_bytes: None,
            hash: Some(ModCatalogueHash {
                algorithm: ModCatalogueHashAlgorithm::Sha256,
                value: "a".repeat(64),
            }),
            version: Some("2".into()),
            region: None,
            notes: None,
            archive_hint: None,
        });
        assert!(record.validate().is_ok());
        let first = serde_json::to_string(&record.clone().canonicalized()).unwrap();
        let second = serde_json::to_string(&record.canonicalized()).unwrap();
        assert_eq!(first, second);
        assert!(first.contains("payload-b"));
        assert!(!first.contains("verified"));
    }

    #[test]
    fn unknown_category_round_trips_as_safe_other_value_and_instructions_stay_text() {
        let category: ModCatalogueCategory =
            serde_json::from_str("\"new_provider_category\"").unwrap();
        assert_eq!(
            category,
            ModCatalogueCategory::Other("new_provider_category".into())
        );
        assert_eq!(
            serde_json::to_string(&category).unwrap(),
            "\"new_provider_category\""
        );
        let record = record(ModCanonicalPlatform::PlayStation3, Vec::new());
        assert_eq!(
            record.instructions.as_deref(),
            Some("Copy according to the later reviewed plan.")
        );
    }
}
