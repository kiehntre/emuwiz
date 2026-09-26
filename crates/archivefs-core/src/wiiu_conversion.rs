//! Read-only planning for verified Wii U WUD/WUX representation conversion.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::wiiu_disc::{WiiUDiscFormat, WiiUDiscInspection, WiiUDiscIssue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiiUConversionDirection {
    WudToWux,
    WuxToWud,
}

impl WiiUConversionDirection {
    pub fn source_format(self) -> WiiUDiscFormat {
        match self {
            Self::WudToWux => WiiUDiscFormat::Wud,
            Self::WuxToWud => WiiUDiscFormat::Wux,
        }
    }

    pub fn target_format(self) -> WiiUDiscFormat {
        match self {
            Self::WudToWux => WiiUDiscFormat::Wux,
            Self::WuxToWud => WiiUDiscFormat::Wud,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiiUConversionReadiness {
    ReadyToPreview,
    ReadyIfToolAvailable,
    VerificationRequired,
    NotReady,
    Unsupported,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WiiUConversionToolStatus {
    Missing,
    VersionUnknown,
    CapabilityUnknown,
    Supported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUConversionToolCapability {
    pub name: String,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub status: WiiUConversionToolStatus,
    pub directions: Vec<WiiUConversionDirection>,
    pub source: String,
    pub modifies_source_in_place: bool,
    pub output_naming: String,
    pub license_provenance: String,
}

impl WiiUConversionToolCapability {
    fn missing(name: &str) -> Self {
        Self {
            name: name.into(),
            path: None,
            version: None,
            status: WiiUConversionToolStatus::Missing,
            directions: Vec::new(),
            source: "bounded PATH lookup; no executable found".into(),
            modifies_source_in_place: false,
            output_naming: "Not established".into(),
            license_provenance: "Not established".into(),
        }
    }

    pub fn supports(&self, direction: WiiUConversionDirection) -> bool {
        self.status == WiiUConversionToolStatus::Supported && self.directions.contains(&direction)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WiiUConversionToolInventory {
    pub tools: Vec<WiiUConversionToolCapability>,
}

fn path_in_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(PathBuf::from)
        .map(|directory| directory.join(name))
        .find(|path| path.is_file())
}

fn discover(name: &str) -> WiiUConversionToolCapability {
    let Some(path) = path_in_path(name) else {
        return WiiUConversionToolCapability::missing(name);
    };
    let probe = Command::new("timeout")
        .args(["2", path.to_string_lossy().as_ref(), "--version"])
        .output();
    let text = probe.ok().map(|output| {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let version = text.as_deref().and_then(|value| {
        value
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .contains(&name.to_ascii_lowercase())
            })
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
    });
    WiiUConversionToolCapability {
        name: name.into(),
        path: Some(path),
        version,
        status: WiiUConversionToolStatus::VersionUnknown,
        directions: Vec::new(),
        source: "bounded --version probe; capability not inferred from filename".into(),
        modifies_source_in_place: false,
        output_naming: "Not established".into(),
        license_provenance: "Not established".into(),
    }
}

pub fn probe_wiiu_conversion_tools() -> WiiUConversionToolInventory {
    WiiUConversionToolInventory {
        tools: vec![discover("JWUDTool"), discover("WudCompress")],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiiUConversionIdentity {
    HashAvailable { algorithm: String, value: String },
    HashMissing,
    HashStale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUConversionSpaceEstimate {
    pub source_bytes: u64,
    pub destination_exact_bytes: Option<u64>,
    pub destination_description: String,
    pub temporary_bytes: Option<u64>,
    pub atomic_duplicate_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub sufficient: Option<bool>,
    pub safety_margin_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUConversionVerificationPlan {
    pub required: bool,
    pub exact_identity_provable: bool,
    pub steps: Vec<String>,
    pub source_identity: WiiUConversionIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WiiUConversionRefusal {
    WrongSourceFormat {
        expected: WiiUDiscFormat,
        actual: WiiUDiscFormat,
    },
    IncompleteSource(Vec<WiiUDiscIssue>),
    SourcePathUnsafe,
    DestinationPathUnsafe,
    DestinationIsSource,
    DestinationExists,
    HashStale,
    HashMissingForVerification,
    ToolUnavailable,
    ToolCapabilityUnproven,
    ToolDoesNotSupportDirection,
    InsufficientDestinationSpace {
        required: u64,
        available: u64,
    },
    OutputSizeUnknown,
    UnsupportedFormat(WiiUDiscFormat),
    AmbiguousSplit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WiiUConversionPlan {
    pub source: PathBuf,
    pub source_parts: Vec<PathBuf>,
    pub destination: PathBuf,
    pub direction: WiiUConversionDirection,
    pub source_format: WiiUDiscFormat,
    pub target_format: WiiUDiscFormat,
    pub source_inspection: WiiUDiscInspection,
    pub source_identity: WiiUConversionIdentity,
    pub tool: Option<WiiUConversionToolCapability>,
    pub readiness: WiiUConversionReadiness,
    pub refusals: Vec<WiiUConversionRefusal>,
    pub warnings: Vec<String>,
    pub space: WiiUConversionSpaceEstimate,
    pub verification: WiiUConversionVerificationPlan,
    pub source_immutable: bool,
    pub keys_required: bool,
    pub provenance: String,
}

#[derive(Debug, Clone)]
pub struct WiiUConversionRequest {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub direction: WiiUConversionDirection,
    pub source_identity: WiiUConversionIdentity,
    pub available_free_space: Option<u64>,
    pub tools: WiiUConversionToolInventory,
}

fn existing_parent(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        if current.is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn safe_regular_source(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn source_parts(report: &WiiUDiscInspection) -> Vec<PathBuf> {
    report
        .structure
        .as_ref()
        .map(|structure| {
            structure
                .parts
                .iter()
                .map(|part| part.path.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn space_for(
    report: &WiiUDiscInspection,
    direction: WiiUConversionDirection,
    available: Option<u64>,
) -> WiiUConversionSpaceEstimate {
    let source = report
        .structure
        .as_ref()
        .map(|structure| structure.physical_container_size_bytes)
        .unwrap_or(0);
    let exact = match direction {
        WiiUConversionDirection::WuxToWud => report
            .structure
            .as_ref()
            .and_then(|structure| structure.logical_disc_size_bytes),
        WiiUConversionDirection::WudToWux => None,
    };
    let description = exact
        .map(|bytes| format!("exact logical WUD size: {bytes} bytes"))
        .unwrap_or_else(|| "WUX output size is unknown until compression is measured".into());
    let temporary = exact.map(|bytes| bytes.saturating_add(source));
    let duplicate = exact.map(|bytes| bytes.saturating_add(source));
    let sufficient = match (available, duplicate) {
        (Some(available), Some(required)) => Some(available >= required),
        _ => None,
    };
    WiiUConversionSpaceEstimate {
        source_bytes: source,
        destination_exact_bytes: exact,
        destination_description: description,
        temporary_bytes: temporary,
        atomic_duplicate_bytes: duplicate,
        available_bytes: available,
        sufficient,
        safety_margin_bytes: source,
    }
}

pub fn plan_wiiu_conversion(request: &WiiUConversionRequest) -> WiiUConversionPlan {
    let report = crate::wiiu_disc::inspect_wii_u_disc(&request.source);
    let mut refusals = Vec::new();
    let mut warnings = Vec::new();
    let expected = request.direction.source_format();
    if report.format != expected {
        refusals.push(match report.format {
            WiiUDiscFormat::Wua => WiiUConversionRefusal::UnsupportedFormat(report.format),
            actual => WiiUConversionRefusal::WrongSourceFormat { expected, actual },
        });
    }
    if !safe_regular_source(&request.source) {
        refusals.push(WiiUConversionRefusal::SourcePathUnsafe);
    }
    if !report.structural_complete {
        refusals.push(WiiUConversionRefusal::IncompleteSource(
            report.issues.clone(),
        ));
    }
    if request.destination == request.source {
        refusals.push(WiiUConversionRefusal::DestinationIsSource);
    }
    if request.destination.exists() {
        refusals.push(WiiUConversionRefusal::DestinationExists);
    }
    if request
        .destination
        .parent()
        .is_some_and(|parent| parent.is_symlink())
    {
        refusals.push(WiiUConversionRefusal::DestinationPathUnsafe);
    }
    let source_identity = request.source_identity.clone();
    match source_identity {
        WiiUConversionIdentity::HashStale => refusals.push(WiiUConversionRefusal::HashStale),
        WiiUConversionIdentity::HashMissing => {
            refusals.push(WiiUConversionRefusal::HashMissingForVerification)
        }
        WiiUConversionIdentity::HashAvailable { .. } => {}
    }
    let tool = request
        .tools
        .tools
        .iter()
        .find(|tool| tool.supports(request.direction))
        .cloned();
    let any_tool = request
        .tools
        .tools
        .iter()
        .find(|tool| tool.path.is_some() && tool.status != WiiUConversionToolStatus::Missing);
    let any_supported_tool = request
        .tools
        .tools
        .iter()
        .any(|tool| tool.status == WiiUConversionToolStatus::Supported);
    if tool.is_none() {
        if any_supported_tool {
            refusals.push(WiiUConversionRefusal::ToolDoesNotSupportDirection);
        } else if any_tool.is_some() {
            refusals.push(WiiUConversionRefusal::ToolCapabilityUnproven);
        } else {
            refusals.push(WiiUConversionRefusal::ToolUnavailable);
        }
    }
    let available = request.available_free_space.or_else(|| {
        existing_parent(&request.destination).and_then(|parent| {
            crate::diagnostics::environment::filesystem_stat(&parent)
                .map(|stat| stat.available_bytes)
        })
    });
    let space = space_for(&report, request.direction, available);
    if let (Some(required), Some(available)) = (space.atomic_duplicate_bytes, space.available_bytes)
        && available < required
    {
        refusals.push(WiiUConversionRefusal::InsufficientDestinationSpace {
            required,
            available,
        });
    }
    if space.destination_exact_bytes.is_none() {
        warnings
            .push("WUX output size is a range/unknown; no compression ratio is invented".into());
        if request.available_free_space.is_some() {
            refusals.push(WiiUConversionRefusal::OutputSizeUnknown);
        }
    }
    let identity_available = matches!(
        request.source_identity,
        WiiUConversionIdentity::HashAvailable { .. }
    );
    let verification = WiiUConversionVerificationPlan {
        required: true,
        exact_identity_provable: identity_available,
        steps: match request.direction {
            WiiUConversionDirection::WudToWux => vec![
                "convert to a separate WUX destination".into(),
                "reconstruct the logical WUD stream from WUX".into(),
                "compare the reconstructed stream with the source WUD identity".into(),
            ],
            WiiUConversionDirection::WuxToWud => vec![
                "convert to a separate WUD destination".into(),
                "hash the reconstructed WUD".into(),
                "compare with the pre-conversion WUD identity".into(),
            ],
        },
        source_identity: request.source_identity.clone(),
    };
    let readiness = if report.format == WiiUDiscFormat::Wua {
        WiiUConversionReadiness::Unsupported
    } else if refusals.iter().any(|refusal| {
        matches!(
            refusal,
            WiiUConversionRefusal::SourcePathUnsafe
                | WiiUConversionRefusal::DestinationPathUnsafe
                | WiiUConversionRefusal::DestinationIsSource
                | WiiUConversionRefusal::DestinationExists
                | WiiUConversionRefusal::HashStale
                | WiiUConversionRefusal::IncompleteSource(_)
                | WiiUConversionRefusal::WrongSourceFormat { .. }
                | WiiUConversionRefusal::UnsupportedFormat(_)
                | WiiUConversionRefusal::InsufficientDestinationSpace { .. }
                | WiiUConversionRefusal::AmbiguousSplit
        )
    }) {
        WiiUConversionReadiness::NotReady
    } else if matches!(request.source_identity, WiiUConversionIdentity::HashMissing) {
        WiiUConversionReadiness::VerificationRequired
    } else if matches!(request.source_identity, WiiUConversionIdentity::HashStale) {
        WiiUConversionReadiness::NotReady
    } else if tool.is_some() && space.destination_exact_bytes.is_some() {
        WiiUConversionReadiness::ReadyToPreview
    } else {
        WiiUConversionReadiness::ReadyIfToolAvailable
    };
    WiiUConversionPlan {
        source: request.source.clone(),
        source_parts: source_parts(&report),
        destination: request.destination.clone(),
        direction: request.direction,
        source_format: report.format,
        target_format: request.direction.target_format(),
        source_inspection: report,
        source_identity,
        tool,
        readiness,
        refusals,
        warnings,
        space,
        verification,
        source_immutable: true,
        keys_required: false,
        provenance: "native Wii U structural inspection plus explicit conversion-plan evidence"
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn make_wux(path: &Path) {
        let mut file = fs::File::create(path).unwrap();
        let mut header = [0_u8; 32];
        header[..4].copy_from_slice(b"WUX0");
        header[4..8].copy_from_slice(&0x1099_d02e_u32.to_le_bytes());
        header[8..12].copy_from_slice(&0x100_u32.to_le_bytes());
        header[16..24].copy_from_slice(&0x100_u64.to_le_bytes());
        file.write_all(&header).unwrap();
        file.write_all(&[0_u8; 252]).unwrap();
        file.write_all(&[0_u8; 256]).unwrap();
    }

    fn tool(direction: WiiUConversionDirection) -> WiiUConversionToolInventory {
        WiiUConversionToolInventory {
            tools: vec![WiiUConversionToolCapability {
                name: "synthetic-converter".into(),
                path: Some("/synthetic/converter".into()),
                version: Some("1.0".into()),
                status: WiiUConversionToolStatus::Supported,
                directions: vec![direction],
                source: "synthetic capability fixture".into(),
                modifies_source_in_place: false,
                output_naming: "explicit destination".into(),
                license_provenance: "synthetic fixture".into(),
            }],
        }
    }

    fn request(source: PathBuf, direction: WiiUConversionDirection) -> WiiUConversionRequest {
        let destination = source.parent().unwrap().join(format!(
            "converted.{}",
            match direction {
                WiiUConversionDirection::WudToWux => "wux",
                WiiUConversionDirection::WuxToWud => "wud",
            }
        ));
        WiiUConversionRequest {
            destination,
            source,
            direction,
            source_identity: WiiUConversionIdentity::HashAvailable {
                algorithm: "sha256".into(),
                value: "synthetic".into(),
            },
            available_free_space: None,
            tools: tool(direction),
        }
    }

    #[test]
    fn plans_both_directions_without_mutating_sources() {
        let directory = tempdir().unwrap();
        let wud = directory.path().join("game.wud");
        fs::write(&wud, [7_u8; 1024]).unwrap();
        let before = fs::read(&wud).unwrap();
        let wud_plan =
            plan_wiiu_conversion(&request(wud.clone(), WiiUConversionDirection::WudToWux));
        assert_eq!(
            wud_plan.readiness,
            WiiUConversionReadiness::ReadyIfToolAvailable
        );
        assert!(wud_plan.verification.exact_identity_provable);

        let wux = directory.path().join("game.wux");
        make_wux(&wux);
        let wux_plan =
            plan_wiiu_conversion(&request(wux.clone(), WiiUConversionDirection::WuxToWud));
        assert_eq!(wux_plan.readiness, WiiUConversionReadiness::ReadyToPreview);
        assert_eq!(wux_plan.space.destination_exact_bytes, Some(256));
        assert_eq!(fs::read(&wud).unwrap(), before);
        assert_eq!(fs::read(&wux).unwrap().len(), 540);
    }

    #[test]
    fn missing_identity_requires_verification_and_stale_identity_refuses() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("game.wux");
        make_wux(&source);
        let mut missing = request(source.clone(), WiiUConversionDirection::WuxToWud);
        missing.source_identity = WiiUConversionIdentity::HashMissing;
        assert_eq!(
            plan_wiiu_conversion(&missing).readiness,
            WiiUConversionReadiness::VerificationRequired
        );
        let mut stale = missing;
        stale.source_identity = WiiUConversionIdentity::HashStale;
        assert_eq!(
            plan_wiiu_conversion(&stale).readiness,
            WiiUConversionReadiness::NotReady
        );
    }

    #[test]
    fn malformed_split_space_and_tool_cases_fail_closed() {
        let directory = tempdir().unwrap();
        let malformed = directory.path().join("bad.wux");
        fs::write(&malformed, b"bad").unwrap();
        let mut bad = request(malformed, WiiUConversionDirection::WuxToWud);
        bad.tools = WiiUConversionToolInventory::default();
        let bad_plan = plan_wiiu_conversion(&bad);
        assert_eq!(bad_plan.readiness, WiiUConversionReadiness::NotReady);
        assert!(
            bad_plan
                .refusals
                .iter()
                .any(|refusal| matches!(refusal, WiiUConversionRefusal::IncompleteSource(_)))
        );

        let split = directory.path().join("title_part1.wud");
        fs::write(&split, [0_u8; 16]).unwrap();
        fs::write(directory.path().join("title_part3.wud"), [0_u8; 16]).unwrap();
        let split_plan = plan_wiiu_conversion(&request(split, WiiUConversionDirection::WudToWux));
        assert!(
            split_plan
                .refusals
                .iter()
                .any(|refusal| matches!(refusal, WiiUConversionRefusal::IncompleteSource(_)))
        );

        let wux = directory.path().join("space.wux");
        make_wux(&wux);
        let mut space = request(wux, WiiUConversionDirection::WuxToWud);
        space.available_free_space = Some(1);
        assert!(
            space
                .tools
                .tools
                .first()
                .unwrap()
                .supports(WiiUConversionDirection::WuxToWud)
        );
        assert!(
            plan_wiiu_conversion(&space)
                .refusals
                .iter()
                .any(|refusal| matches!(
                    refusal,
                    WiiUConversionRefusal::InsufficientDestinationSpace { .. }
                ))
        );

        let unsupported = directory.path().join("unsupported.wud");
        fs::write(&unsupported, [0_u8; 32]).unwrap();
        let unsupported_plan = plan_wiiu_conversion(&WiiUConversionRequest {
            source: unsupported,
            destination: directory.path().join("unsupported.wux"),
            direction: WiiUConversionDirection::WudToWux,
            source_identity: WiiUConversionIdentity::HashAvailable {
                algorithm: "sha256".into(),
                value: "synthetic".into(),
            },
            available_free_space: None,
            tools: tool(WiiUConversionDirection::WuxToWud),
        });
        assert!(
            unsupported_plan
                .refusals
                .contains(&WiiUConversionRefusal::ToolDoesNotSupportDirection)
        );
    }
}
