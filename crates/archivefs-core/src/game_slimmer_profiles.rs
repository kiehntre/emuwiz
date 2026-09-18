//! Strict, local-first knowledge for future Game Slimmer operations.
//!
//! Profiles are evidence-backed data only. This module deliberately has no
//! filesystem writer, process launcher, downloader, or destructive executor.
//! A profile can describe a permitted operation, but authorization still
//! requires a trusted profile and no `UnknownUnsafe` rule.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};

use crate::psp_reversible_shrink::PspShrinkInspection;

pub const PROFILE_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROFILE_BYTES: usize = 512 * 1024;
pub const MAX_PROFILES_PER_BUNDLE: usize = 4096;
pub const MAX_RULES_PER_PROFILE: usize = 256;
pub const MAX_EVIDENCE_PER_PROFILE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    Invalid(String),
    DuplicateProfileId(String),
    Json(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid Game Slimmer profile: {message}"),
            Self::DuplicateProfileId(id) => write!(f, "duplicate Game Slimmer profile ID: {id}"),
            Self::Json(message) => write!(f, "invalid Game Slimmer profile JSON: {message}"),
        }
    }
}

impl std::error::Error for ProfileError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlimmerPlatform {
    Psp,
    Ps2,
    Xbox,
    GameCube,
    Wii,
    ScummVm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum GameIdentityBinding {
    Psp {
        disc_id: String,
    },
    Ps2 {
        serial: String,
        executable_crc: Option<String>,
    },
    Xbox {
        title_id: String,
    },
    GameCube {
        game_id: String,
    },
    Wii {
        game_id: String,
    },
    ScummVm {
        game_id: String,
        engine: String,
        variant: Option<String>,
    },
}

impl GameIdentityBinding {
    fn platform(&self) -> SlimmerPlatform {
        match self {
            Self::Psp { .. } => SlimmerPlatform::Psp,
            Self::Ps2 { .. } => SlimmerPlatform::Ps2,
            Self::Xbox { .. } => SlimmerPlatform::Xbox,
            Self::GameCube { .. } => SlimmerPlatform::GameCube,
            Self::Wii { .. } => SlimmerPlatform::Wii,
            Self::ScummVm { .. } => SlimmerPlatform::ScummVm,
        }
    }

    fn validate(&self) -> Result<(), ProfileError> {
        match self {
            Self::Psp { disc_id }
            | Self::Xbox { title_id: disc_id }
            | Self::GameCube { game_id: disc_id }
            | Self::Wii { game_id: disc_id } => validate_token(disc_id, "identity value"),
            Self::Ps2 {
                serial,
                executable_crc,
            } => {
                validate_token(serial, "PS2 serial")?;
                if let Some(crc) = executable_crc {
                    validate_hex(crc, 8, "PS2 executable CRC")?;
                }
                Ok(())
            }
            Self::ScummVm {
                game_id,
                engine,
                variant,
            } => {
                validate_token(game_id, "ScummVM game ID")?;
                validate_token(engine, "ScummVM engine")?;
                if let Some(variant) = variant {
                    validate_token(variant, "ScummVM variant")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RevisionConstraint {
    pub revision: Option<String>,
    pub build: Option<String>,
    pub source_sha256: Option<String>,
}

impl RevisionConstraint {
    fn validate(&self) -> Result<(), ProfileError> {
        if let Some(value) = &self.revision {
            validate_token(value, "revision")?;
        }
        if let Some(value) = &self.build {
            validate_token(value, "build")?;
        }
        if let Some(value) = &self.source_sha256 {
            validate_hex(value, 64, "source SHA-256")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct TargetConfiguration {
    pub language: Option<String>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
}

impl TargetConfiguration {
    fn validate(&self) -> Result<(), ProfileError> {
        if let Some(language) = &self.language {
            validate_token(language, "target language")?;
        }
        if self.options.len() > 64 {
            return Err(ProfileError::Invalid("too many target options".into()));
        }
        for (key, value) in &self.options {
            validate_token(key, "target option name")?;
            validate_token(value, "target option value")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "selector", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssetSelector {
    ExactPath { path: String },
    PathPrefix { path: String },
}

impl AssetSelector {
    fn validate(&self) -> Result<(), ProfileError> {
        let path = match self {
            Self::ExactPath { path } | Self::PathPrefix { path } => path,
        };
        validate_safe_relative_path(path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetCategory {
    Audio,
    Subtitle,
    LanguageData,
    Update,
    Dummy,
    Partition,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOperation {
    Retain,
    Remove,
    ReplaceWithValidatedDummy,
    ZeroPayloadPreserveLayout,
    DropPartition,
    RecompressOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafetyLevel {
    ProvenSafe,
    ConditionallySafe,
    UnknownUnsafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationRequirement {
    StructuralVerified,
    IdentityRetained,
    BootCheckPassed,
    LanguageSelectionChecked,
    FirstPlayableSceneReached,
    ManualPlaytestRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRule {
    pub selector: AssetSelector,
    pub asset_category: AssetCategory,
    pub operation: ProfileOperation,
    pub expected_size: Option<u64>,
    pub expected_sha256: Option<String>,
    pub expected_signature: Option<String>,
    pub safety: SafetyLevel,
    pub reason: String,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default)]
    pub required_verification: Vec<VerificationRequirement>,
}

impl ProfileRule {
    fn validate(&self, evidence_ids: &[&str]) -> Result<(), ProfileError> {
        self.selector.validate()?;
        if self.expected_sha256.is_some() {
            validate_hex(
                self.expected_sha256.as_deref().unwrap_or_default(),
                64,
                "asset SHA-256",
            )?;
        }
        if let Some(signature) = &self.expected_signature {
            validate_token(signature, "asset signature")?;
        }
        if self.reason.trim().is_empty() || self.reason.len() > 4096 {
            return Err(ProfileError::Invalid(
                "rule reason is empty or too long".into(),
            ));
        }
        if self
            .evidence_refs
            .iter()
            .any(|id| !evidence_ids.contains(&id.as_str()))
        {
            return Err(ProfileError::Invalid(
                "rule references evidence that is not present".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    VerifiedGameIdentity,
    ExactSourceRevisionHash,
    ReverseEngineeredContainerMapping,
    ExecutableResourceReference,
    UpstreamToolDocumentation,
    CommunityTestedFinding,
    LocalEmuWizExperiment,
    BootTestResult,
    ManualPlayTestResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceConfidence {
    Observed,
    Corroborated,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileEvidence {
    pub evidence_id: String,
    pub kind: EvidenceKind,
    pub confidence: EvidenceConfidence,
    pub source: String,
    pub description: String,
    pub exact_source_sha256: Option<String>,
}

impl ProfileEvidence {
    fn validate(&self) -> Result<(), ProfileError> {
        validate_token(&self.evidence_id, "evidence ID")?;
        validate_token(&self.source, "evidence source")?;
        if self.description.trim().is_empty() || self.description.len() > 4096 {
            return Err(ProfileError::Invalid(
                "evidence description is empty or too long".into(),
            ));
        }
        if let Some(hash) = &self.exact_source_sha256 {
            validate_hex(hash, 64, "evidence source SHA-256")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    BuiltIn,
    UserLocal,
    ImportedCommunity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileTrust {
    Untrusted,
    Reviewed,
    Trusted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileProvenance {
    pub origin: ProfileOrigin,
    pub trust: ProfileTrust,
    pub author: String,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

impl ProfileProvenance {
    fn validate(&self) -> Result<(), ProfileError> {
        if self.origin == ProfileOrigin::ImportedCommunity && self.trust == ProfileTrust::Trusted {
            return Err(ProfileError::Invalid(
                "imported community profiles cannot become trusted merely by parsing".into(),
            ));
        }
        for (value, label) in [
            (&self.author, "provenance author"),
            (&self.source, "provenance source"),
            (&self.created_at, "created metadata"),
            (&self.updated_at, "updated metadata"),
        ] {
            validate_token(value, label)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlimmerProfile {
    pub profile_id: String,
    pub platform: SlimmerPlatform,
    pub game_identity: GameIdentityBinding,
    #[serde(default)]
    pub revision: RevisionConstraint,
    pub profile_version: u32,
    pub target: TargetConfiguration,
    pub rules: Vec<ProfileRule>,
    pub evidence: Vec<ProfileEvidence>,
    pub verification_requirements: Vec<VerificationRequirement>,
    pub provenance: ProfileProvenance,
}

impl SlimmerProfile {
    pub fn validate(&self) -> Result<(), ProfileError> {
        validate_token(&self.profile_id, "profile ID")?;
        if self.profile_version == 0 {
            return Err(ProfileError::Invalid(
                "profile_version must be non-zero".into(),
            ));
        }
        if self.platform != self.game_identity.platform() {
            return Err(ProfileError::Invalid(
                "platform does not match game_identity".into(),
            ));
        }
        self.game_identity.validate()?;
        self.revision.validate()?;
        self.target.validate()?;
        if self.rules.is_empty() || self.rules.len() > MAX_RULES_PER_PROFILE {
            return Err(ProfileError::Invalid("invalid rule count".into()));
        }
        if self.evidence.is_empty() || self.evidence.len() > MAX_EVIDENCE_PER_PROFILE {
            return Err(ProfileError::Invalid("invalid evidence count".into()));
        }
        let mut evidence_ids = Vec::with_capacity(self.evidence.len());
        for evidence in &self.evidence {
            evidence.validate()?;
            if evidence_ids.contains(&evidence.evidence_id.as_str()) {
                return Err(ProfileError::Invalid("duplicate evidence ID".into()));
            }
            evidence_ids.push(evidence.evidence_id.as_str());
        }
        for rule in &self.rules {
            rule.validate(&evidence_ids)?;
        }
        self.provenance.validate()
    }

    /// A profile is never executable data. Authorization is intentionally
    /// stricter than parsing and matching: imported/untrusted profiles and
    /// any UnknownUnsafe rule cannot authorize a future modifier.
    pub fn can_authorize_modification(&self) -> bool {
        self.provenance.trust == ProfileTrust::Trusted
            && self
                .rules
                .iter()
                .all(|rule| rule.safety != SafetyLevel::UnknownUnsafe)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBundle {
    pub schema_version: u32,
    pub profiles: Vec<SlimmerProfile>,
}

impl ProfileBundle {
    pub fn validate(&self) -> Result<(), ProfileError> {
        if self.schema_version != PROFILE_SCHEMA_VERSION {
            return Err(ProfileError::Invalid(format!(
                "unsupported schema_version {}; expected {}",
                self.schema_version, PROFILE_SCHEMA_VERSION
            )));
        }
        if self.profiles.len() > MAX_PROFILES_PER_BUNDLE {
            return Err(ProfileError::Invalid("too many profiles".into()));
        }
        let mut ids = Vec::with_capacity(self.profiles.len());
        for profile in &self.profiles {
            profile.validate()?;
            if ids.iter().any(|id: &&str| *id == profile.profile_id) {
                return Err(ProfileError::DuplicateProfileId(profile.profile_id.clone()));
            }
            ids.push(profile.profile_id.as_str());
        }
        Ok(())
    }

    pub fn from_json(input: &str) -> Result<Self, ProfileError> {
        if input.len() > MAX_PROFILE_BYTES {
            return Err(ProfileError::Invalid(
                "profile document exceeds size limit".into(),
            ));
        }
        let bundle: Self =
            serde_json::from_str(input).map_err(|error| ProfileError::Json(error.to_string()))?;
        bundle.validate()?;
        Ok(bundle)
    }

    /// Struct field order and BTreeMap ordering make this serialization
    /// deterministic for stable local diffs and export/import receipts.
    pub fn to_json(&self) -> Result<String, ProfileError> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|error| ProfileError::Json(error.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedGameIdentity {
    /// The caller must only set this after identity evidence has been checked.
    pub verified: bool,
    pub platform: SlimmerPlatform,
    pub game_identity: GameIdentityBinding,
    pub revision: Option<String>,
    pub build: Option<String>,
    pub source_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MatchRequest {
    pub language: Option<String>,
    pub options: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileMatchResult {
    ExactProfileMatch(Vec<String>),
    ConditionalProfileMatch(Vec<String>),
    NoProfile,
    Conflict(Vec<String>),
}

pub fn match_profiles(
    profiles: &[SlimmerProfile],
    identity: &VerifiedGameIdentity,
    request: &MatchRequest,
) -> ProfileMatchResult {
    if !identity.verified {
        return ProfileMatchResult::NoProfile;
    }
    let mut matches = Vec::new();
    for profile in profiles {
        if profile.platform != identity.platform
            || profile.game_identity != identity.game_identity
            || !revision_matches(&profile.revision, identity)
            || !target_matches(&profile.target, request)
        {
            continue;
        }
        matches.push(profile);
    }
    if matches.is_empty() {
        return ProfileMatchResult::NoProfile;
    }
    if matches.len() > 1 {
        return ProfileMatchResult::Conflict(
            matches
                .iter()
                .map(|profile| profile.profile_id.clone())
                .collect(),
        );
    }
    let profile = matches[0];
    let ids = vec![profile.profile_id.clone()];
    if profile
        .rules
        .iter()
        .any(|rule| rule.safety == SafetyLevel::ConditionallySafe)
    {
        ProfileMatchResult::ConditionalProfileMatch(ids)
    } else {
        ProfileMatchResult::ExactProfileMatch(ids)
    }
}

/// Read-only bridge for the existing PSP analyzer. The Disc ID and revision
/// must come from verified PSP evidence; this function never derives either
/// from the filename or compression output.
pub fn match_psp_inspection_profile(
    profile: &SlimmerProfile,
    inspection: &PspShrinkInspection,
    verified_disc_id: &str,
    verified_revision: Option<&str>,
    request: &MatchRequest,
) -> ProfileMatchResult {
    let identity = VerifiedGameIdentity {
        verified: true,
        platform: SlimmerPlatform::Psp,
        game_identity: GameIdentityBinding::Psp {
            disc_id: verified_disc_id.to_string(),
        },
        revision: verified_revision.map(str::to_string),
        build: None,
        source_sha256: Some(inspection.source_sha256.clone()),
    };
    match_profiles(std::slice::from_ref(profile), &identity, request)
}

fn revision_matches(constraint: &RevisionConstraint, identity: &VerifiedGameIdentity) -> bool {
    constraint
        .revision
        .as_deref()
        .is_none_or(|value| Some(value) == identity.revision.as_deref())
        && constraint
            .build
            .as_deref()
            .is_none_or(|value| Some(value) == identity.build.as_deref())
        && constraint
            .source_sha256
            .as_deref()
            .is_none_or(|value| Some(value) == identity.source_sha256.as_deref())
}

fn target_matches(target: &TargetConfiguration, request: &MatchRequest) -> bool {
    target.language.as_deref() == request.language.as_deref() && target.options == request.options
}

fn validate_token(value: &str, label: &str) -> Result<(), ProfileError> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return Err(ProfileError::Invalid(format!(
            "{label} is empty, invalid, or too long"
        )));
    }
    Ok(())
}

fn validate_hex(value: &str, length: usize, label: &str) -> Result<(), ProfileError> {
    if value.len() != length || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ProfileError::Invalid(format!(
            "{label} must be {length} hex characters"
        )));
    }
    Ok(())
}

fn validate_safe_relative_path(value: &str) -> Result<(), ProfileError> {
    if value.is_empty() || value.len() > 512 || value.contains('\\') || value.contains(':') {
        return Err(ProfileError::Invalid(
            "asset selector path is unsafe".into(),
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::RootDir | Component::ParentDir | Component::Prefix(_)
            )
        })
    {
        return Err(ProfileError::Invalid(
            "asset selector path must be relative and bounded".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn profile(
        profile_id: &str,
        disc_id: &str,
        revision: Option<&str>,
        safety: SafetyLevel,
    ) -> SlimmerProfile {
        SlimmerProfile {
            profile_id: profile_id.into(),
            platform: SlimmerPlatform::Psp,
            game_identity: GameIdentityBinding::Psp {
                disc_id: disc_id.into(),
            },
            revision: RevisionConstraint {
                revision: revision.map(str::to_string),
                build: None,
                source_sha256: None,
            },
            profile_version: 1,
            target: TargetConfiguration {
                language: Some("English".into()),
                options: BTreeMap::new(),
            },
            rules: vec![ProfileRule {
                selector: AssetSelector::ExactPath {
                    path: "PSP_GAME/USRDIR/voice.pak".into(),
                },
                asset_category: AssetCategory::Audio,
                operation: ProfileOperation::Retain,
                expected_size: Some(12),
                expected_sha256: None,
                expected_signature: Some("fixture-signature".into()),
                safety,
                reason: "synthetic fixture only".into(),
                evidence_refs: vec!["identity".into()],
                required_verification: vec![VerificationRequirement::IdentityRetained],
            }],
            evidence: vec![ProfileEvidence {
                evidence_id: "identity".into(),
                kind: EvidenceKind::VerifiedGameIdentity,
                confidence: EvidenceConfidence::Verified,
                source: "synthetic-test".into(),
                description: "synthetic verified PSP identity".into(),
                exact_source_sha256: None,
            }],
            verification_requirements: vec![VerificationRequirement::StructuralVerified],
            provenance: ProfileProvenance {
                origin: ProfileOrigin::BuiltIn,
                trust: ProfileTrust::Trusted,
                author: "EmuWiz test".into(),
                source: "local fixture".into(),
                created_at: "2026-09-18".into(),
                updated_at: "2026-09-18".into(),
            },
        }
    }

    fn identity(disc_id: &str, revision: Option<&str>) -> VerifiedGameIdentity {
        VerifiedGameIdentity {
            verified: true,
            platform: SlimmerPlatform::Psp,
            game_identity: GameIdentityBinding::Psp {
                disc_id: disc_id.into(),
            },
            revision: revision.map(str::to_string),
            build: None,
            source_sha256: None,
        }
    }

    #[test]
    fn valid_profile_parses() {
        let profile = profile(
            "psp.fixture",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        let bundle = ProfileBundle {
            schema_version: PROFILE_SCHEMA_VERSION,
            profiles: vec![profile],
        };
        let json = bundle.to_json().unwrap();
        assert_eq!(ProfileBundle::from_json(&json).unwrap(), bundle);
    }

    #[test]
    fn export_import_is_deterministic() {
        let bundle = ProfileBundle {
            schema_version: PROFILE_SCHEMA_VERSION,
            profiles: vec![profile(
                "psp.fixture",
                "ULUS-00000",
                Some("1.00"),
                SafetyLevel::ProvenSafe,
            )],
        };
        let first = bundle.to_json().unwrap();
        let second = ProfileBundle::from_json(&first).unwrap().to_json().unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn identity_and_revision_mismatches_fail_closed() {
        let profile = profile(
            "psp.fixture",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        let request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        assert_eq!(
            match_profiles(
                std::slice::from_ref(&profile),
                &identity("ULUS-99999", Some("1.00")),
                &request
            ),
            ProfileMatchResult::NoProfile
        );
        assert_eq!(
            match_profiles(
                std::slice::from_ref(&profile),
                &identity("ULUS-00000", Some("2.00")),
                &request
            ),
            ProfileMatchResult::NoProfile
        );
    }

    #[test]
    fn exact_identity_matches() {
        let profile = profile(
            "psp.fixture",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        let request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        assert_eq!(
            match_profiles(&[profile], &identity("ULUS-00000", Some("1.00")), &request),
            ProfileMatchResult::ExactProfileMatch(vec!["psp.fixture".into()])
        );
    }

    #[test]
    fn conditional_language_rule_is_enforced() {
        let profile = profile(
            "psp.fixture",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ConditionallySafe,
        );
        let english = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        let japanese = MatchRequest {
            language: Some("Japanese".into()),
            options: BTreeMap::new(),
        };
        assert!(matches!(
            match_profiles(
                std::slice::from_ref(&profile),
                &identity("ULUS-00000", Some("1.00")),
                &english
            ),
            ProfileMatchResult::ConditionalProfileMatch(_)
        ));
        assert_eq!(
            match_profiles(
                std::slice::from_ref(&profile),
                &identity("ULUS-00000", Some("1.00")),
                &japanese
            ),
            ProfileMatchResult::NoProfile
        );
    }

    #[test]
    fn unknown_unsafe_cannot_authorize() {
        let profile = profile(
            "psp.fixture",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::UnknownUnsafe,
        );
        assert!(!profile.can_authorize_modification());
    }

    #[test]
    fn duplicate_ids_and_malformed_documents_fail_closed() {
        let p = profile(
            "duplicate",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        let duplicate = ProfileBundle {
            schema_version: PROFILE_SCHEMA_VERSION,
            profiles: vec![p.clone(), p],
        };
        assert!(matches!(
            duplicate.validate(),
            Err(ProfileError::DuplicateProfileId(_))
        ));
        assert!(ProfileBundle::from_json("{\"schema_version\":1,\"profiles\":[}").is_err());
        assert!(ProfileBundle::from_json("{\"schema_version\":2,\"profiles\":[]}").is_err());
    }

    #[test]
    fn unsafe_paths_and_absolute_paths_are_rejected() {
        for path in [
            "../voice.pak",
            "/tmp/voice.pak",
            "C:/voice.pak",
            "PSP_GAME\\voice.pak",
        ] {
            let mut p = profile("path", "ULUS-00000", Some("1.00"), SafetyLevel::ProvenSafe);
            p.rules[0].selector = AssetSelector::ExactPath { path: path.into() };
            assert!(p.validate().is_err(), "path should be rejected: {path}");
        }
    }

    #[test]
    fn imported_profile_trust_is_distinct_from_validity() {
        let mut p = profile(
            "imported",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        p.provenance.origin = ProfileOrigin::ImportedCommunity;
        p.provenance.trust = ProfileTrust::Untrusted;
        assert!(p.validate().is_ok());
        assert!(!p.can_authorize_modification());
    }

    #[test]
    fn imported_trusted_claim_is_rejected() {
        let mut p = profile(
            "imported",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        p.provenance.origin = ProfileOrigin::ImportedCommunity;
        assert!(p.validate().is_err());
    }

    #[test]
    fn conflicting_matching_profiles_are_not_silently_selected() {
        let first = profile("first", "ULUS-00000", Some("1.00"), SafetyLevel::ProvenSafe);
        let second = profile(
            "second",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        let request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        assert_eq!(
            match_profiles(
                &[first, second],
                &identity("ULUS-00000", Some("1.00")),
                &request
            ),
            ProfileMatchResult::Conflict(vec!["first".into(), "second".into()])
        );
    }

    #[test]
    fn filename_only_identity_does_not_match() {
        let profile = profile(
            "psp.fixture",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        let request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        assert_eq!(
            match_profiles(
                &[profile],
                &identity("ULUS-99999 GAME NAME", Some("1.00")),
                &request,
            ),
            ProfileMatchResult::NoProfile
        );
    }

    #[test]
    fn profile_has_no_executable_command_field() {
        let json = r#"{"schema_version":1,"profiles":[] ,"command":"rm -rf /"}"#;
        assert!(ProfileBundle::from_json(json).is_err());
    }

    #[test]
    fn unverified_identity_cannot_match() {
        let p = profile(
            "psp.fixture",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        let mut unverified = identity("ULUS-00000", Some("1.00"));
        unverified.verified = false;
        let request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        assert_eq!(
            match_profiles(&[p], &unverified, &request),
            ProfileMatchResult::NoProfile
        );
    }

    #[test]
    fn constrained_source_hash_must_match() {
        let mut p = profile(
            "psp.hash",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        p.revision.source_sha256 = Some("a".repeat(64));
        let request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        let mut matching = identity("ULUS-00000", Some("1.00"));
        matching.source_sha256 = Some("a".repeat(64));
        assert!(matches!(
            match_profiles(&[p.clone()], &matching, &request),
            ProfileMatchResult::ExactProfileMatch(_)
        ));
        matching.source_sha256 = Some("b".repeat(64));
        assert_eq!(
            match_profiles(&[p], &matching, &request),
            ProfileMatchResult::NoProfile
        );
    }

    #[test]
    fn target_options_are_part_of_match() {
        let mut p = profile(
            "psp.options",
            "ULUS-00000",
            Some("1.00"),
            SafetyLevel::ProvenSafe,
        );
        p.target.options.insert("voice".into(), "original".into());
        let mut request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        assert_eq!(
            match_profiles(
                &[p.clone()],
                &identity("ULUS-00000", Some("1.00")),
                &request
            ),
            ProfileMatchResult::NoProfile
        );
        request.options.insert("voice".into(), "original".into());
        assert!(matches!(
            match_profiles(&[p], &identity("ULUS-00000", Some("1.00")), &request),
            ProfileMatchResult::ExactProfileMatch(_)
        ));
    }

    #[test]
    fn psp_analyzer_inspection_is_consumed_read_only() {
        let profile = profile("psp.fixture", "ULUS-00000", None, SafetyLevel::ProvenSafe);
        let inspection = PspShrinkInspection {
            source: PathBuf::from("fixture.iso"),
            source_size: 2048,
            source_sha256: "a".repeat(64),
            identity_evidence: vec!["verified PARAM.SFO DISC_ID".into()],
        };
        let request = MatchRequest {
            language: Some("English".into()),
            options: BTreeMap::new(),
        };
        assert_eq!(
            match_psp_inspection_profile(&profile, &inspection, "ULUS-00000", None, &request),
            ProfileMatchResult::ExactProfileMatch(vec!["psp.fixture".into()])
        );
        assert_eq!(inspection.source_sha256, "a".repeat(64));
    }
}
