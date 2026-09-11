//! Approval-bound managed installation primitives.
//!
//! Emulator policy modules are deliberately not part of this layer. They
//! construct a [`BootstrapContext`] and provide policy evidence to the
//! generic plan/execution boundary.

pub(crate) mod installer;
pub(crate) mod model;
pub(crate) mod process;
pub(crate) mod safety;

pub mod pcsx2;
pub mod ppsspp;

mod legacy;

pub use legacy::{
    initialize_managed_appimage, initialize_managed_appimage_with_timeout,
    managed_appimage_is_initialized, ManagedAppImageBootstrapError, ManagedAppImageBootstrapKind,
    ManagedAppImageBootstrapReceipt,
};
pub use model::{
    BootstrapApproval, BootstrapContext, BootstrapError, BootstrapExecutor, BootstrapInspection,
    BootstrapOutcome, BootstrapStep, BootstrapTarget, EmulatorBootstrapPlan, RemediationKind,
};

use crate::emulator_download::EmulatorDownloadTransport;

/// Inspect the bound environment without network access or mutation.
pub fn inspect(context: &BootstrapContext) -> Result<BootstrapInspection, BootstrapError> {
    model::inspect(context)
}

/// Build an immutable plan from already-resolved release metadata. Resolving
/// release metadata remains the responsibility of the policy layer; this
/// function itself has no side effects.
pub fn plan(
    context: BootstrapContext,
    download: Option<crate::emulator_download::EmulatorDownloadPlan>,
    steps: Vec<BootstrapStep>,
) -> Result<EmulatorBootstrapPlan, BootstrapError> {
    model::plan(context, download, steps)
}

/// Revalidate an approved plan. Policy code supplies the side effects only
/// after this generic boundary has accepted the approval and snapshots.
pub fn execute<E: BootstrapExecutor>(
    plan: &EmulatorBootstrapPlan,
    approval: BootstrapApproval,
    executor: &E,
    transport: Option<&dyn EmulatorDownloadTransport>,
) -> Result<BootstrapOutcome, BootstrapError> {
    model::execute(plan, approval, executor, transport)
}

#[cfg(test)]
mod tests;
