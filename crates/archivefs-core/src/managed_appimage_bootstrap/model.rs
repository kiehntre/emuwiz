use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::emulator_download::{EmulatorDownloadPlan, EmulatorDownloadTransport};

use super::{installer, safety};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapTarget {
    Ppsspp,
    Pcsx2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemediationKind {
    DetectOnly,
    ConfigureExistingInstall,
    ManagedPortableInstall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStep {
    DownloadAndInstallVerifiedRelease,
    VerifyInstalledDigestAndProvenance,
    VerifyExistingExecutableBinding,
    ProbeVersion,
    RunNativeFirstSetup,
    RediscoverProfileAndReadiness,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapApproval {
    NotApproved,
    ExplicitlyApproved { plan_binding: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapContext {
    pub target: BootstrapTarget,
    pub root: PathBuf,
    pub destination: PathBuf,
    pub executable: Option<PathBuf>,
    pub release: Option<String>,
    pub asset_url: Option<String>,
    pub expected_digest: Option<String>,
    pub source: String,
    pub host: String,
    pub arch: String,
    pub environment: BTreeMap<String, String>,
    pub filesystem: BTreeMap<PathBuf, Option<String>>,
    pub policy_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapInspection {
    pub target: BootstrapTarget,
    pub destination: PathBuf,
    pub executable: Option<PathBuf>,
    pub release: Option<String>,
    pub digest: Option<String>,
    pub filesystem: BTreeMap<PathBuf, Option<String>>,
    pub policy_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatorBootstrapPlan {
    pub context: BootstrapContext,
    pub before: BootstrapInspection,
    pub download: Option<EmulatorDownloadPlan>,
    pub remediation: RemediationKind,
    pub steps: Vec<BootstrapStep>,
}

impl EmulatorBootstrapPlan {
    pub fn approval(&self) -> BootstrapApproval {
        BootstrapApproval::ExplicitlyApproved {
            plan_binding: self.binding(),
        }
    }

    fn binding(&self) -> String {
        use sha2::{Digest, Sha256};

        let mut hash = Sha256::new();
        hash.update(format!("{:?}", self).as_bytes());
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootstrapOutcome {
    pub release: Option<String>,
    pub destination: PathBuf,
    pub inspection: BootstrapInspection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapError {
    NotApproved,
    StalePlan(&'static str),
    UnsafePath(String),
    MissingDigest,
    Publication(String),
    Process(String),
    Policy(String),
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotApproved => f.write_str("explicit approval of this plan is required"),
            Self::StalePlan(detail) => write!(f, "setup changed since review: {detail}"),
            Self::UnsafePath(detail)
            | Self::Publication(detail)
            | Self::Process(detail)
            | Self::Policy(detail) => f.write_str(detail),
            Self::MissingDigest => f.write_str("automatic setup requires a published SHA-256"),
        }
    }
}

impl std::error::Error for BootstrapError {}

pub trait BootstrapExecutor {
    fn execute(&self, plan: &EmulatorBootstrapPlan) -> Result<(), BootstrapError>;
    fn final_inspection(
        &self,
        context: &BootstrapContext,
    ) -> Result<BootstrapInspection, BootstrapError>;
}

pub fn inspect(context: &BootstrapContext) -> Result<BootstrapInspection, BootstrapError> {
    safety::validate_context(context)?;
    let digest = match context.executable.as_deref() {
        Some(path) => safety::file_hash(path, safety::MAX_HASH_BYTES)?,
        None => None,
    };
    Ok(BootstrapInspection {
        target: context.target,
        destination: context.destination.clone(),
        executable: context.executable.clone(),
        release: context.release.clone(),
        digest,
        filesystem: context
            .filesystem
            .keys()
            .map(|path| {
                safety::file_hash(path, safety::MAX_HASH_BYTES).map(|digest| (path.clone(), digest))
            })
            .collect::<Result<_, _>>()?,
        policy_fingerprint: context.policy_fingerprint.clone(),
    })
}

pub fn plan(
    context: BootstrapContext,
    download: Option<EmulatorDownloadPlan>,
    steps: Vec<BootstrapStep>,
) -> Result<EmulatorBootstrapPlan, BootstrapError> {
    let before = inspect(&context)?;
    if let Some(download) = &download {
        if download.destination_path != context.destination {
            return Err(BootstrapError::StalePlan(
                "destination does not match download plan",
            ));
        }
        if download.expected_sha256.is_none() {
            return Err(BootstrapError::MissingDigest);
        }
    }
    let remediation = if download.is_some() {
        RemediationKind::ManagedPortableInstall
    } else if context.executable.is_some() {
        RemediationKind::DetectOnly
    } else {
        RemediationKind::ConfigureExistingInstall
    };
    Ok(EmulatorBootstrapPlan {
        context,
        before,
        download,
        remediation,
        steps,
    })
}

pub fn execute<E: BootstrapExecutor>(
    plan: &EmulatorBootstrapPlan,
    approval: BootstrapApproval,
    executor: &E,
    transport: Option<&dyn EmulatorDownloadTransport>,
) -> Result<BootstrapOutcome, BootstrapError> {
    let BootstrapApproval::ExplicitlyApproved { plan_binding } = approval else {
        return Err(BootstrapError::NotApproved);
    };
    if plan_binding != plan.binding() {
        return Err(BootstrapError::StalePlan(
            "approval does not match this plan",
        ));
    }
    let current = inspect(&plan.context)?;
    if current != plan.before {
        return Err(BootstrapError::StalePlan(
            "filesystem, executable or policy evidence",
        ));
    }
    if plan.download.is_some() {
        let Some(transport) = transport else {
            return Err(BootstrapError::Policy(
                "release revalidation transport is missing".into(),
            ));
        };
        installer::revalidate_release(plan, transport)?;
    }
    executor.execute(plan)?;
    let final_inspection = executor.final_inspection(&plan.context)?;
    Ok(BootstrapOutcome {
        release: final_inspection.release.clone(),
        destination: final_inspection.destination.clone(),
        inspection: final_inspection,
    })
}
