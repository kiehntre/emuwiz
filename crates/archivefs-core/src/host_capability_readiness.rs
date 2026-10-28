//! Conservative, read-only host capability readiness projection.
//!
//! Hard emulator requirements are evaluated separately from performance
//! guidance.  Unknown evidence remains unknown; this module never tunes a
//! host, changes drivers, benchmarks hardware, or writes system state.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCapabilityEvidenceState {
    Known,
    Unknown,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostCapabilityEvidence {
    pub source: String,
    pub state: HostCapabilityEvidenceState,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CpuCapabilities {
    pub architecture: Option<String>,
    pub available_features: BTreeSet<String>,
    pub unavailable_features: BTreeSet<String>,
    pub evidence: Vec<HostCapabilityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphicsApiCapabilities {
    pub api: String,
    pub version: Option<(u32, u32)>,
    pub extensions: BTreeSet<String>,
    pub evidence_state: HostCapabilityEvidenceState,
    pub evidence: Vec<HostCapabilityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GpuCapabilities {
    pub adapters: Vec<String>,
    pub apis: BTreeMap<String, GraphicsApiCapabilities>,
    pub evidence: Vec<HostCapabilityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostCapabilities {
    pub architecture: Option<String>,
    pub cpu: CpuCapabilities,
    pub gpu: GpuCapabilities,
    pub memory_bytes: Option<u64>,
    pub evidence: Vec<HostCapabilityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphicsApiRequirement {
    pub api: String,
    pub minimum_version: Option<(u32, u32)>,
    pub required_extensions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EmulatorHostRequirements {
    pub emulator: String,
    pub supported_architectures: BTreeSet<String>,
    pub required_cpu_features: BTreeSet<String>,
    pub graphics: Option<GraphicsApiRequirement>,
    pub minimum_memory_bytes: Option<u64>,
    /// Informational constraints; these never produce hard blockers.
    pub performance_guidance: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostCapabilityReadiness {
    Ready,
    RequirementMissing,
    RequirementsUnknown,
    UnsupportedArchitecture,
    GraphicsApiMissing,
    CpuFeatureMissing,
    MemoryInsufficient,
    PerformanceUnknown,
    Stale,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostCapabilityIssue {
    pub readiness: HostCapabilityReadiness,
    pub requirement: String,
    pub reason: String,
    pub evidence: Vec<HostCapabilityEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostCapabilityReadinessReport {
    pub emulator: String,
    pub readiness: HostCapabilityReadiness,
    pub issues: Vec<HostCapabilityIssue>,
    pub performance_guidance: Vec<String>,
    pub read_only: bool,
}

impl Default for HostCapabilities {
    fn default() -> Self {
        Self {
            architecture: None,
            cpu: CpuCapabilities {
                architecture: None,
                available_features: BTreeSet::new(),
                unavailable_features: BTreeSet::new(),
                evidence: Vec::new(),
            },
            gpu: GpuCapabilities {
                adapters: Vec::new(),
                apis: BTreeMap::new(),
                evidence: Vec::new(),
            },
            memory_bytes: None,
            evidence: Vec::new(),
        }
    }
}

/// Gather only low-risk operating-system facts. Graphics capabilities remain
/// unknown unless a trusted adapter supplies them; no GPU probe is attempted.
pub fn detect_host_capabilities() -> HostCapabilities {
    let architecture = Some(std::env::consts::ARCH.to_string());
    let mut available_features = BTreeSet::new();
    #[cfg(target_arch = "x86_64")]
    {
        for (name, available) in [
            ("sse", std::is_x86_feature_detected!("sse")),
            ("sse2", std::is_x86_feature_detected!("sse2")),
            ("avx", std::is_x86_feature_detected!("avx")),
            ("avx2", std::is_x86_feature_detected!("avx2")),
        ] {
            if available {
                available_features.insert(name.to_string());
            }
        }
    }
    let cpu = CpuCapabilities {
        architecture: architecture.clone(),
        available_features,
        unavailable_features: BTreeSet::new(),
        evidence: vec![HostCapabilityEvidence {
            source: "Rust target and CPU feature detection".into(),
            state: HostCapabilityEvidenceState::Known,
            detail: "reported host architecture and conservatively detected CPU flags".into(),
        }],
    };
    let memory_bytes = read_linux_memory_bytes();
    HostCapabilities {
        architecture,
        cpu,
        gpu: GpuCapabilities {
            adapters: Vec::new(),
            apis: BTreeMap::new(),
            evidence: vec![HostCapabilityEvidence {
                source: "host capability planner".into(),
                state: HostCapabilityEvidenceState::Unknown,
                detail: "graphics API evidence was not supplied by a trusted adapter".into(),
            }],
        },
        memory_bytes,
        evidence: vec![HostCapabilityEvidence {
            source: "host capability planner".into(),
            state: if memory_bytes.is_some() {
                HostCapabilityEvidenceState::Known
            } else {
                HostCapabilityEvidenceState::Unknown
            },
            detail: "read-only host capability snapshot".into(),
        }],
    }
}

#[cfg(target_os = "linux")]
fn read_linux_memory_bytes() -> Option<u64> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kilobytes = contents.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        (fields.next() == Some("MemTotal:")).then(|| fields.next()?.parse::<u64>().ok())?
    })?;
    kilobytes.checked_mul(1024)
}

#[cfg(not(target_os = "linux"))]
fn read_linux_memory_bytes() -> Option<u64> {
    None
}

fn issue(
    readiness: HostCapabilityReadiness,
    requirement: impl Into<String>,
    reason: impl Into<String>,
    evidence: Vec<HostCapabilityEvidence>,
) -> HostCapabilityIssue {
    HostCapabilityIssue {
        readiness,
        requirement: requirement.into(),
        reason: reason.into(),
        evidence,
    }
}

pub fn assess_host_capabilities(
    requirements: &EmulatorHostRequirements,
    host: &HostCapabilities,
) -> HostCapabilityReadinessReport {
    let mut issues = Vec::new();
    let mut checked_requirement = false;

    if !requirements.supported_architectures.is_empty() {
        checked_requirement = true;
        match host.architecture.as_deref() {
            Some(architecture) if requirements.supported_architectures.contains(architecture) => {}
            Some(architecture) => issues.push(issue(
                HostCapabilityReadiness::UnsupportedArchitecture,
                "supported host architecture",
                format!("host architecture {architecture} is not supported"),
                host.cpu.evidence.clone(),
            )),
            None => issues.push(issue(
                HostCapabilityReadiness::RequirementsUnknown,
                "supported host architecture",
                "host architecture evidence is unavailable",
                host.evidence.clone(),
            )),
        }
    }

    for feature in &requirements.required_cpu_features {
        checked_requirement = true;
        if host.cpu.available_features.contains(feature) {
            continue;
        }
        let readiness = if host.cpu.unavailable_features.contains(feature) {
            HostCapabilityReadiness::CpuFeatureMissing
        } else {
            HostCapabilityReadiness::RequirementsUnknown
        };
        issues.push(issue(
            readiness,
            format!("CPU feature {feature}"),
            if readiness == HostCapabilityReadiness::CpuFeatureMissing {
                "required CPU feature is absent"
            } else {
                "CPU feature evidence is unknown"
            },
            host.cpu.evidence.clone(),
        ));
    }

    if let Some(graphics) = &requirements.graphics {
        checked_requirement = true;
        match host.gpu.apis.get(&graphics.api) {
            None => issues.push(issue(
                HostCapabilityReadiness::RequirementsUnknown,
                format!("{} graphics capability", graphics.api),
                "graphics API evidence is unavailable",
                host.gpu.evidence.clone(),
            )),
            Some(capability)
                if capability.evidence_state == HostCapabilityEvidenceState::Unknown =>
            {
                issues.push(issue(
                    HostCapabilityReadiness::RequirementsUnknown,
                    format!("{} graphics capability", graphics.api),
                    "graphics API evidence is unknown, not proven missing",
                    capability.evidence.clone(),
                ));
            }
            Some(capability) if capability.evidence_state == HostCapabilityEvidenceState::Stale => {
                issues.push(issue(
                    HostCapabilityReadiness::Stale,
                    format!("{} graphics capability", graphics.api),
                    "graphics capability evidence is stale and current applicability is unproven",
                    capability.evidence.clone(),
                ));
            }
            Some(capability) => {
                if let Some(minimum) = graphics.minimum_version {
                    match capability.version {
                        None => issues.push(issue(
                            HostCapabilityReadiness::RequirementsUnknown,
                            format!("{} version", graphics.api),
                            "graphics API version evidence is unavailable",
                            capability.evidence.clone(),
                        )),
                        Some(version) if version < minimum => issues.push(issue(
                            HostCapabilityReadiness::GraphicsApiMissing,
                            format!("{} version", graphics.api),
                            "graphics API version is below the hard minimum",
                            capability.evidence.clone(),
                        )),
                        Some(_) => {}
                    }
                }
                for extension in &graphics.required_extensions {
                    if !capability.extensions.contains(extension) {
                        issues.push(issue(
                            HostCapabilityReadiness::GraphicsApiMissing,
                            format!("{} extension {extension}", graphics.api),
                            "required graphics extension is absent",
                            capability.evidence.clone(),
                        ));
                    }
                }
            }
        }
    }

    if let Some(minimum) = requirements.minimum_memory_bytes {
        checked_requirement = true;
        match host.memory_bytes {
            Some(actual) if actual < minimum => issues.push(issue(
                HostCapabilityReadiness::MemoryInsufficient,
                "minimum host memory",
                format!("host reports {actual} bytes, below required {minimum}"),
                host.evidence.clone(),
            )),
            None => issues.push(issue(
                HostCapabilityReadiness::RequirementsUnknown,
                "minimum host memory",
                "host memory evidence is unavailable",
                host.evidence.clone(),
            )),
            Some(_) => {}
        }
    }

    let readiness = if issues.iter().any(|issue| {
        matches!(
            issue.readiness,
            HostCapabilityReadiness::UnsupportedArchitecture
                | HostCapabilityReadiness::GraphicsApiMissing
                | HostCapabilityReadiness::CpuFeatureMissing
                | HostCapabilityReadiness::MemoryInsufficient
        )
    }) {
        HostCapabilityReadiness::RequirementMissing
    } else if issues
        .iter()
        .any(|issue| issue.readiness == HostCapabilityReadiness::Stale)
    {
        HostCapabilityReadiness::Stale
    } else if !checked_requirement
        || issues
            .iter()
            .any(|issue| issue.readiness == HostCapabilityReadiness::RequirementsUnknown)
    {
        HostCapabilityReadiness::RequirementsUnknown
    } else if !requirements.performance_guidance.is_empty() {
        HostCapabilityReadiness::PerformanceUnknown
    } else {
        HostCapabilityReadiness::Ready
    };

    HostCapabilityReadinessReport {
        emulator: requirements.emulator.clone(),
        readiness,
        issues,
        performance_guidance: requirements.performance_guidance.clone(),
        read_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostCapabilities {
        HostCapabilities {
            architecture: Some("x86_64".into()),
            cpu: CpuCapabilities {
                architecture: Some("x86_64".into()),
                available_features: ["avx2".into()].into_iter().collect(),
                unavailable_features: BTreeSet::new(),
                evidence: vec![HostCapabilityEvidence {
                    source: "synthetic CPU evidence".into(),
                    state: HostCapabilityEvidenceState::Known,
                    detail: "synthetic".into(),
                }],
            },
            gpu: GpuCapabilities {
                adapters: vec!["synthetic GPU".into()],
                apis: BTreeMap::new(),
                evidence: Vec::new(),
            },
            memory_bytes: Some(16 * 1024 * 1024 * 1024),
            evidence: Vec::new(),
        }
    }

    fn requirements() -> EmulatorHostRequirements {
        EmulatorHostRequirements {
            emulator: "Synthetic Emulator".into(),
            supported_architectures: ["x86_64".into()].into_iter().collect(),
            required_cpu_features: BTreeSet::new(),
            graphics: None,
            minimum_memory_bytes: None,
            performance_guidance: Vec::new(),
        }
    }

    #[test]
    fn cpu_avx2_present_satisfies_requirement() {
        let mut requirements = requirements();
        requirements.required_cpu_features.insert("avx2".into());
        assert_eq!(
            assess_host_capabilities(&requirements, &host()).readiness,
            HostCapabilityReadiness::Ready
        );
    }

    #[test]
    fn cpu_avx2_absent_is_blocked() {
        let mut requirements = requirements();
        requirements.required_cpu_features.insert("avx2".into());
        let mut host = host();
        host.cpu.available_features.clear();
        host.cpu.unavailable_features.insert("avx2".into());
        assert_eq!(
            assess_host_capabilities(&requirements, &host).readiness,
            HostCapabilityReadiness::RequirementMissing
        );
    }

    #[test]
    fn missing_graphics_extension_is_blocked() {
        let mut requirements = requirements();
        requirements.graphics = Some(GraphicsApiRequirement {
            api: "Vulkan".into(),
            minimum_version: Some((1, 2)),
            required_extensions: ["VK_synthetic_extension".into()].into_iter().collect(),
        });
        let mut host = host();
        host.gpu.apis.insert(
            "Vulkan".into(),
            GraphicsApiCapabilities {
                api: "Vulkan".into(),
                version: Some((1, 3)),
                extensions: BTreeSet::new(),
                evidence_state: HostCapabilityEvidenceState::Known,
                evidence: Vec::new(),
            },
        );
        assert_eq!(
            assess_host_capabilities(&requirements, &host).readiness,
            HostCapabilityReadiness::RequirementMissing
        );
    }

    #[test]
    fn graphics_version_evidence_distinguishes_unknown_missing_and_satisfied() {
        let mut requirements = requirements();
        requirements.graphics = Some(GraphicsApiRequirement {
            api: "Vulkan".into(),
            minimum_version: Some((1, 2)),
            required_extensions: BTreeSet::new(),
        });
        for (version, expected) in [
            (None, HostCapabilityReadiness::RequirementsUnknown),
            (Some((1, 1)), HostCapabilityReadiness::RequirementMissing),
            (Some((1, 2)), HostCapabilityReadiness::Ready),
            (Some((1, 3)), HostCapabilityReadiness::Ready),
        ] {
            let mut host = host();
            let evidence = vec![HostCapabilityEvidence {
                source: "synthetic graphics adapter".into(),
                state: HostCapabilityEvidenceState::Known,
                detail: "API observed; version supplied separately".into(),
            }];
            host.gpu.apis.insert(
                "Vulkan".into(),
                GraphicsApiCapabilities {
                    api: "Vulkan".into(),
                    version,
                    extensions: BTreeSet::new(),
                    evidence_state: HostCapabilityEvidenceState::Known,
                    evidence: evidence.clone(),
                },
            );
            let report = assess_host_capabilities(&requirements, &host);
            assert_eq!(report.readiness, expected, "version: {version:?}");
            assert!(report.read_only);
            match version {
                None => {
                    assert_eq!(report.issues.len(), 1);
                    assert_eq!(report.issues[0].readiness, expected);
                    assert_eq!(report.issues[0].evidence, evidence);
                }
                Some((1, 1)) => {
                    assert_eq!(report.issues.len(), 1);
                    assert_eq!(
                        report.issues[0].readiness,
                        HostCapabilityReadiness::GraphicsApiMissing
                    );
                }
                Some(_) => assert!(report.issues.is_empty()),
            }
        }
    }

    #[test]
    fn unknown_graphics_version_does_not_hide_a_missing_extension() {
        let mut requirements = requirements();
        requirements.graphics = Some(GraphicsApiRequirement {
            api: "Vulkan".into(),
            minimum_version: Some((1, 2)),
            required_extensions: ["VK_synthetic_extension".into()].into_iter().collect(),
        });
        let mut host = host();
        host.gpu.apis.insert(
            "Vulkan".into(),
            GraphicsApiCapabilities {
                api: "Vulkan".into(),
                version: None,
                extensions: BTreeSet::new(),
                evidence_state: HostCapabilityEvidenceState::Known,
                evidence: Vec::new(),
            },
        );
        let report = assess_host_capabilities(&requirements, &host);
        assert_eq!(
            report.readiness,
            HostCapabilityReadiness::RequirementMissing
        );
        assert_eq!(report.issues.len(), 2);
        assert_eq!(
            report.issues[0].readiness,
            HostCapabilityReadiness::RequirementsUnknown
        );
        assert_eq!(
            report.issues[1].readiness,
            HostCapabilityReadiness::GraphicsApiMissing
        );
    }

    #[test]
    fn unknown_graphics_is_unknown_not_blocked() {
        let mut requirements = requirements();
        requirements.graphics = Some(GraphicsApiRequirement {
            api: "Vulkan".into(),
            minimum_version: None,
            required_extensions: BTreeSet::new(),
        });
        assert_eq!(
            assess_host_capabilities(&requirements, &host()).readiness,
            HostCapabilityReadiness::RequirementsUnknown
        );
    }

    #[test]
    fn stale_graphics_evidence_is_not_ready() {
        let mut requirements = requirements();
        requirements.graphics = Some(GraphicsApiRequirement {
            api: "Vulkan".into(),
            minimum_version: None,
            required_extensions: BTreeSet::new(),
        });
        let mut host = host();
        host.gpu.apis.insert(
            "Vulkan".into(),
            GraphicsApiCapabilities {
                api: "Vulkan".into(),
                version: Some((1, 3)),
                extensions: BTreeSet::new(),
                evidence_state: HostCapabilityEvidenceState::Stale,
                evidence: Vec::new(),
            },
        );
        assert_eq!(
            assess_host_capabilities(&requirements, &host).readiness,
            HostCapabilityReadiness::Stale
        );
    }

    #[test]
    fn architecture_and_memory_failures_are_explicit() {
        let mut requirements = requirements();
        requirements.supported_architectures = ["aarch64".into()].into_iter().collect();
        assert_eq!(
            assess_host_capabilities(&requirements, &host()).readiness,
            HostCapabilityReadiness::RequirementMissing
        );
        requirements.supported_architectures = ["x86_64".into()].into_iter().collect();
        requirements.minimum_memory_bytes = Some(32 * 1024 * 1024 * 1024);
        assert_eq!(
            assess_host_capabilities(&requirements, &host()).readiness,
            HostCapabilityReadiness::RequirementMissing
        );
    }

    #[test]
    fn performance_guidance_is_not_a_hard_failure() {
        let mut requirements = requirements();
        requirements
            .performance_guidance
            .push("higher GPU headroom may help".into());
        let report = assess_host_capabilities(&requirements, &host());
        assert_eq!(
            report.readiness,
            HostCapabilityReadiness::PerformanceUnknown
        );
        assert!(report.issues.is_empty());
    }

    #[test]
    fn no_proven_requirements_remains_conservative_and_read_only() {
        let requirements = EmulatorHostRequirements {
            emulator: "Unknown Emulator".into(),
            supported_architectures: BTreeSet::new(),
            required_cpu_features: BTreeSet::new(),
            graphics: None,
            minimum_memory_bytes: None,
            performance_guidance: Vec::new(),
        };
        let report = assess_host_capabilities(&requirements, &host());
        assert_eq!(
            report.readiness,
            HostCapabilityReadiness::RequirementsUnknown
        );
        assert!(report.read_only);
    }
}
