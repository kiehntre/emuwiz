//! Safe, read-only conversion planning.
//!
//! Planning is deliberately separate from execution.  A plan describes the
//! exact structured argv, safety contract, space needs, collision state and
//! verification work.  It never opens a destination for writing.

use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionOperationClass {
    LosslessRecompress,
    ReversibleTrim,
    ContentModified,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionVerificationExpectation {
    ByteExactRestoreExpected,
    SemanticLosslessOnly,
    NotReversible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionVerificationLevel {
    NotRun,
    StructuralVerified,
    SemanticEquivalent,
    ByteExactRestoreVerified,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionPlatform {
    GameCube,
    Wii,
    WiiU,
    Switch,
    Psp,
    Optical,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionFormat {
    Iso,
    Rvz,
    Wia,
    Chd,
    Cso,
    Wua,
    Nsz,
    Xcz,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitMode {
    Raw,
    DefaultScrub,
    SelectUpdate,
    SelectData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChdMediaKind {
    Cd,
    Dvd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionRequest {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub platform: ConversionPlatform,
    pub source_format: ConversionFormat,
    pub target_format: ConversionFormat,
    pub wit_mode: Option<WitMode>,
    pub available_free_space: Option<u64>,
    pub required_free_space: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationPlan {
    pub expectation: ConversionVerificationExpectation,
    pub steps: Vec<String>,
    pub restore_output_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionPlan {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub platform: ConversionPlatform,
    pub source_format: ConversionFormat,
    pub target_format: ConversionFormat,
    pub backend: String,
    pub backend_version: Option<String>,
    /// Structured argv tokens. No shell command is constructed.
    pub argv: Vec<String>,
    pub operation_class: ConversionOperationClass,
    pub verification: VerificationPlan,
    pub source_size: u64,
    pub required_free_space: u64,
    pub collision: bool,
    pub source_immutable: bool,
    pub content_changes: bool,
    pub data_discarded: bool,
    pub output_type: ConversionFormat,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionPlanError {
    SourceMissing(PathBuf),
    SourceDestinationSame(PathBuf),
    OutputExists(PathBuf),
    InsufficientFreeSpace { required: u64, available: u64 },
    Unsupported(String),
    ToolUnavailable(String),
}

impl fmt::Display for ConversionPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceMissing(path) => write!(f, "source does not exist: {}", path.display()),
            Self::SourceDestinationSame(path) => {
                write!(
                    f,
                    "source and destination must be different: {}",
                    path.display()
                )
            }
            Self::OutputExists(path) => write!(f, "output already exists: {}", path.display()),
            Self::InsufficientFreeSpace {
                required,
                available,
            } => write!(
                f,
                "insufficient free space: {available} bytes available, {required} required"
            ),
            Self::Unsupported(reason) => write!(f, "unsupported conversion: {reason}"),
            Self::ToolUnavailable(tool) => write!(f, "conversion tool unavailable: {tool}"),
        }
    }
}

impl std::error::Error for ConversionPlanError {}

fn validate_request(request: &ConversionRequest) -> Result<u64, ConversionPlanError> {
    let source = fs::metadata(&request.source)
        .map_err(|_| ConversionPlanError::SourceMissing(request.source.clone()))?;
    if request.source == request.destination {
        return Err(ConversionPlanError::SourceDestinationSame(
            request.source.clone(),
        ));
    }
    if request.destination.exists() {
        return Err(ConversionPlanError::OutputExists(
            request.destination.clone(),
        ));
    }
    if let Some(available) = request.available_free_space
        && available < request.required_free_space
    {
        return Err(ConversionPlanError::InsufficientFreeSpace {
            required: request.required_free_space,
            available,
        });
    }
    Ok(source.len())
}

struct PlanDetails {
    backend: String,
    argv: Vec<String>,
    operation_class: ConversionOperationClass,
    verification: VerificationPlan,
    content_changes: bool,
    data_discarded: bool,
    warnings: Vec<String>,
}

fn base_plan(
    request: &ConversionRequest,
    source_size: u64,
    details: PlanDetails,
) -> ConversionPlan {
    ConversionPlan {
        source: request.source.clone(),
        destination: request.destination.clone(),
        platform: request.platform,
        source_format: request.source_format,
        target_format: request.target_format,
        backend: details.backend,
        backend_version: None,
        argv: details.argv,
        operation_class: details.operation_class,
        verification: details.verification,
        source_size,
        required_free_space: request.required_free_space,
        collision: false,
        source_immutable: true,
        content_changes: details.content_changes,
        data_discarded: details.data_discarded,
        output_type: request.target_format,
        warnings: details.warnings,
    }
}

pub fn plan_wit_iso_to_wia(
    request: &ConversionRequest,
) -> Result<ConversionPlan, ConversionPlanError> {
    if request.source_format != ConversionFormat::Iso
        || request.target_format != ConversionFormat::Wia
    {
        return Err(ConversionPlanError::Unsupported(
            "WIT planner requires ISO to WIA".into(),
        ));
    }
    let source_size = validate_request(request)?;
    let mode = request.wit_mode.unwrap_or(WitMode::DefaultScrub);
    let mut argv = vec!["wit".into(), "copy".into()];
    let (class, changes, discarded, verification, warnings) = match mode {
        WitMode::Raw => {
            argv.push("--raw".into());
            (
                ConversionOperationClass::LosslessRecompress,
                false,
                false,
                VerificationPlan {
                    expectation: ConversionVerificationExpectation::ByteExactRestoreExpected,
                    steps: vec![
                        "restore WIA to a temporary ISO".into(),
                        "SHA-256 the restored ISO and compare with the source".into(),
                    ],
                    restore_output_required: true,
                },
                Vec::new(),
            )
        }
        WitMode::DefaultScrub => (
            ConversionOperationClass::ContentModified,
            true,
            true,
            VerificationPlan {
                expectation: ConversionVerificationExpectation::NotReversible,
                steps: vec!["verify the modified WIA structurally".into()],
                restore_output_required: false,
            },
            vec!["WIT default copy may scrub/discard data; it is not preservation-safe".into()],
        ),
        WitMode::SelectUpdate => {
            argv.extend(["--psel".into(), "-update".into()]);
            (
                ConversionOperationClass::ContentModified,
                true,
                true,
                VerificationPlan {
                    expectation: ConversionVerificationExpectation::NotReversible,
                    steps: vec!["verify the modified partition set structurally".into()],
                    restore_output_required: false,
                },
                vec!["UPDATE partition selection changes content".into()],
            )
        }
        WitMode::SelectData => {
            argv.extend(["--psel".into(), "data".into()]);
            (
                ConversionOperationClass::ContentModified,
                true,
                true,
                VerificationPlan {
                    expectation: ConversionVerificationExpectation::NotReversible,
                    steps: vec!["verify the modified partition set structurally".into()],
                    restore_output_required: false,
                },
                vec!["DATA-only partition selection changes content".into()],
            )
        }
    };
    argv.extend([
        request.source.to_string_lossy().into_owned(),
        request.destination.to_string_lossy().into_owned(),
    ]);
    Ok(base_plan(
        request,
        source_size,
        PlanDetails {
            backend: "wit".into(),
            argv,
            operation_class: class,
            verification,
            content_changes: changes,
            data_discarded: discarded,
            warnings,
        },
    ))
}

pub fn plan_psp_iso_to_cso(
    request: &ConversionRequest,
) -> Result<ConversionPlan, ConversionPlanError> {
    if request.platform != ConversionPlatform::Psp
        || request.source_format != ConversionFormat::Iso
        || request.target_format != ConversionFormat::Cso
    {
        return Err(ConversionPlanError::Unsupported(
            "PSP planner requires PSP ISO to CSO".into(),
        ));
    }
    let source_size = validate_request(request)?;
    let verification = VerificationPlan {
        expectation: ConversionVerificationExpectation::ByteExactRestoreExpected,
        steps: vec![
            "validate the CSO header and blocks".into(),
            "restore a temporary ISO and compare SHA-256 with the source".into(),
        ],
        restore_output_required: true,
    };
    Ok(base_plan(
        request,
        source_size,
        PlanDetails {
            backend: "EmuWiz PSP CSO v1".into(),
            argv: vec![
                "internal".into(),
                "psp-iso-to-cso".into(),
                request.source.to_string_lossy().into_owned(),
                request.destination.to_string_lossy().into_owned(),
            ],
            operation_class: ConversionOperationClass::LosslessRecompress,
            verification,
            content_changes: false,
            data_discarded: false,
            warnings: Vec::new(),
        },
    ))
}

pub fn plan_chd_conversion(
    request: &ConversionRequest,
    media_kind: ChdMediaKind,
) -> Result<ConversionPlan, ConversionPlanError> {
    if request.target_format != ConversionFormat::Chd
        || !matches!(
            request.source_format,
            ConversionFormat::Iso | ConversionFormat::Unknown
        )
    {
        return Err(ConversionPlanError::Unsupported(
            "CHD planner requires an explicitly classified optical source".into(),
        ));
    }
    let source_size = validate_request(request)?;
    let (mode, extract_mode, expectation) = match media_kind {
        ChdMediaKind::Dvd => (
            "createdvd",
            "extractdvd",
            ConversionVerificationExpectation::ByteExactRestoreExpected,
        ),
        ChdMediaKind::Cd => (
            "createcd",
            "extractcd",
            ConversionVerificationExpectation::SemanticLosslessOnly,
        ),
    };
    let verification = VerificationPlan {
        expectation,
        steps: vec![
            format!("restore with chdman {extract_mode}"),
            "compare the appropriate byte or canonical optical fingerprint".into(),
        ],
        restore_output_required: true,
    };
    Ok(base_plan(
        request,
        source_size,
        PlanDetails {
            backend: "chdman".into(),
            argv: vec![
                "chdman".into(),
                mode.into(),
                request.source.to_string_lossy().into_owned(),
                request.destination.to_string_lossy().into_owned(),
            ],
            operation_class: ConversionOperationClass::LosslessRecompress,
            verification,
            content_changes: false,
            data_discarded: false,
            warnings: vec![
                "media topology must be established before selecting createcd or createdvd".into(),
            ],
        },
    ))
}

pub fn classify_rvz_route() -> ConversionOperationClass {
    ConversionOperationClass::LosslessRecompress
}

pub fn classify_rom_converto_route(
    tool_available: bool,
) -> Result<ConversionOperationClass, ConversionPlanError> {
    if tool_available {
        Ok(ConversionOperationClass::LosslessRecompress)
    } else {
        Err(ConversionPlanError::ToolUnavailable("rom-converto".into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionReceipt {
    pub source: PathBuf,
    pub source_sha256: Option<String>,
    pub source_size: u64,
    pub output: PathBuf,
    pub output_sha256: Option<String>,
    pub output_size: Option<u64>,
    pub backend: String,
    pub tool_version: Option<String>,
    pub argv: Vec<String>,
    pub operation_class: ConversionOperationClass,
    pub timestamp_unix_seconds: u64,
    pub byte_savings: Option<i64>,
    pub content_changed: bool,
    pub verification: ConversionVerificationLevel,
    pub restored_sha256: Option<String>,
    pub source_untouched: bool,
}

impl ConversionReceipt {
    pub fn from_plan(plan: &ConversionPlan, tool_version: Option<String>) -> Self {
        Self {
            source: plan.source.clone(),
            source_sha256: None,
            source_size: plan.source_size,
            output: plan.destination.clone(),
            output_sha256: None,
            output_size: None,
            backend: plan.backend.clone(),
            tool_version,
            argv: plan.argv.clone(),
            operation_class: plan.operation_class,
            timestamp_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            byte_savings: None,
            content_changed: plan.content_changes,
            verification: ConversionVerificationLevel::NotRun,
            restored_sha256: None,
            source_untouched: plan.source_immutable,
        }
    }

    pub fn record_byte_exact_restore(&mut self, restored_sha256: String) {
        self.restored_sha256 = Some(restored_sha256);
        self.verification = if self.source_sha256 == self.restored_sha256 {
            ConversionVerificationLevel::ByteExactRestoreVerified
        } else {
            ConversionVerificationLevel::Failed
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn request(dir: &Path, target: ConversionFormat) -> ConversionRequest {
        let source = dir.join("source with spaces.iso");
        fs::write(&source, [1u8, 2, 3, 4]).unwrap();
        ConversionRequest {
            source,
            destination: dir.join("output with spaces.img"),
            platform: ConversionPlatform::Wii,
            source_format: ConversionFormat::Iso,
            target_format: target,
            wit_mode: None,
            available_free_space: Some(1024),
            required_free_space: 10,
        }
    }

    #[test]
    fn raw_wit_is_the_only_lossless_wia_route() {
        let dir = tempfile::tempdir().unwrap();
        let mut raw = request(dir.path(), ConversionFormat::Wia);
        raw.wit_mode = Some(WitMode::Raw);
        let plan = plan_wit_iso_to_wia(&raw).unwrap();
        assert_eq!(
            plan.operation_class,
            ConversionOperationClass::LosslessRecompress
        );
        assert_eq!(plan.argv[2], "--raw");
        assert!(plan.verification.restore_output_required);

        let scrub = request(dir.path(), ConversionFormat::Wia);
        let scrub = plan_wit_iso_to_wia(&scrub).unwrap();
        assert_eq!(
            scrub.operation_class,
            ConversionOperationClass::ContentModified
        );
        assert!(scrub.data_discarded);
    }

    #[test]
    fn wit_partition_selection_is_content_modified() {
        let dir = tempfile::tempdir().unwrap();
        for mode in [WitMode::SelectUpdate, WitMode::SelectData] {
            let mut request = request(dir.path(), ConversionFormat::Wia);
            request.wit_mode = Some(mode);
            request.destination = dir.path().join(format!("{mode:?}.wia"));
            let plan = plan_wit_iso_to_wia(&request).unwrap();
            assert_eq!(
                plan.operation_class,
                ConversionOperationClass::ContentModified
            );
            assert!(plan.content_changes);
            assert!(!plan.verification.restore_output_required);
        }
    }

    #[test]
    fn psp_route_retains_existing_byte_exact_contract() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path(), ConversionFormat::Cso);
        request.platform = ConversionPlatform::Psp;
        request.destination = dir.path().join("out.cso");
        let plan = plan_psp_iso_to_cso(&request).unwrap();
        assert_eq!(
            plan.operation_class,
            ConversionOperationClass::LosslessRecompress
        );
        assert_eq!(
            plan.verification.expectation,
            ConversionVerificationExpectation::ByteExactRestoreExpected
        );
    }

    #[test]
    fn collision_and_in_place_requests_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path(), ConversionFormat::Wia);
        fs::write(&request.destination, [9u8]).unwrap();
        assert!(matches!(
            plan_wit_iso_to_wia(&request),
            Err(ConversionPlanError::OutputExists(_))
        ));
        request.destination = request.source.clone();
        fs::remove_file(&request.source).unwrap();
        fs::write(&request.source, [1u8]).unwrap();
        assert!(matches!(
            plan_wit_iso_to_wia(&request),
            Err(ConversionPlanError::SourceDestinationSame(_))
        ));
    }

    #[test]
    fn receipt_never_claims_restore_until_hashes_match() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path(), ConversionFormat::Wia);
        request.wit_mode = Some(WitMode::Raw);
        let plan = plan_wit_iso_to_wia(&request).unwrap();
        let mut receipt = ConversionReceipt::from_plan(&plan, Some("wit 3.01a".into()));
        receipt.source_sha256 = Some("source".into());
        receipt.record_byte_exact_restore("different".into());
        assert_eq!(receipt.verification, ConversionVerificationLevel::Failed);
        receipt.record_byte_exact_restore("source".into());
        assert_eq!(
            receipt.verification,
            ConversionVerificationLevel::ByteExactRestoreVerified
        );
        assert_eq!(receipt.tool_version.as_deref(), Some("wit 3.01a"));
        assert!(receipt.source_untouched);
    }

    #[test]
    fn rvz_is_lossless_but_unavailable_tool_is_not_hidden() {
        assert_eq!(
            classify_rvz_route(),
            ConversionOperationClass::LosslessRecompress
        );
        assert!(matches!(
            classify_rom_converto_route(false),
            Err(ConversionPlanError::ToolUnavailable(_))
        ));
    }

    #[test]
    fn free_space_and_unsupported_routes_fail_before_execution() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path(), ConversionFormat::Wia);
        request.available_free_space = Some(1);
        assert!(matches!(
            plan_wit_iso_to_wia(&request),
            Err(ConversionPlanError::InsufficientFreeSpace { .. })
        ));
        request.available_free_space = Some(1024);
        request.target_format = ConversionFormat::Unknown;
        assert!(matches!(
            plan_wit_iso_to_wia(&request),
            Err(ConversionPlanError::Unsupported(_))
        ));
    }

    #[test]
    fn chd_planning_requires_explicit_media_topology() {
        let dir = tempfile::tempdir().unwrap();
        let request = request(dir.path(), ConversionFormat::Chd);
        let cd = plan_chd_conversion(&request, ChdMediaKind::Cd).unwrap();
        assert_eq!(cd.argv[1], "createcd");
        assert_eq!(
            cd.verification.expectation,
            ConversionVerificationExpectation::SemanticLosslessOnly
        );
        let dvd = plan_chd_conversion(&request, ChdMediaKind::Dvd).unwrap();
        assert_eq!(dvd.argv[1], "createdvd");
        assert_eq!(
            dvd.verification.expectation,
            ConversionVerificationExpectation::ByteExactRestoreExpected
        );
    }
}
