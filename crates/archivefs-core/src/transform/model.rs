//! Data model of the transformation engine: immutable plans, the steps they
//! compile to, the durable events the journal records, and the outcomes the
//! engine may report.
//!
//! Every type here is serialised into the journal. Changing a variant's shape
//! is a format change: bump [`PLAN_VERSION`] / the journal version and keep
//! older builds failing closed (unknown variants do not deserialise).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::catalogue_health::SourceRootBinding;
use crate::dat::rename_apply::model::ObjectIdentity;

pub const PLAN_FORMAT: &str = "emuwiz-transform-plan";
pub const PLAN_VERSION: u32 = 1;
pub const ENGINE: &str = "ouroboros-1";

/// (device, inode) of a directory or file.
pub type DirId = (u64, u64);

/// A library root the plan is authorised to touch, bound to the root's
/// directory and filesystem identity observed at planning time
/// ([`SourceRootBinding`]). A root that is later unmounted, replaced or
/// reached on a different filesystem no longer matches, and the engine fails
/// closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootBinding {
    pub path: PathBuf,
    pub binding: SourceRootBinding,
    /// The catalogue's `source_folders.id` for this root.
    pub source_folder_id: i64,
}

/// How long a retired original is kept. Relocation completion never deletes
/// an original; deletion would be a separate, explicitly approved operation
/// that does not exist in this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPolicy {
    /// Keep the original byte-for-byte in its private retirement directory,
    /// on its own filesystem, until a person explicitly releases it.
    RetainUntilReleased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// Same filesystem: one `renameat2(RENAME_NOREPLACE)`; the inode moves.
    AtomicRename,
    /// Different filesystems: stage a copy beside the destination, verify it
    /// by full re-read, publish it no-clobber, commit the catalogue, then
    /// retire (never delete) the original on its own filesystem.
    CopyVerifyRetire,
}

/// One durable step. Each step is idempotent: it is decided by probing the
/// world, so a repeated recovery never invents a second change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Re-proves every planning precondition immediately before mutation.
    CheckPreconditions,
    /// Creates the private (0700) staging directory beside the destination.
    CreatePrivateDir { dir: PathBuf, root: PathBuf },
    /// Copies the pinned source into `staged` and syncs file and directory.
    CopyToStaging { staged: PathBuf },
    /// Re-reads the staged copy end to end and compares it with the plan's
    /// SHA-256 and size.
    VerifyStaged { staged: PathBuf },
    /// No-clobber rename of a file the plan owns, with directory sync.
    /// `expect` says whose identity the moved object must have; `roots` are
    /// the only roots the two parents may be resolved beneath.
    Rename {
        from: PathBuf,
        to: PathBuf,
        roots: Vec<PathBuf>,
        expect: Expect,
    },
    /// Compare-and-swap of the catalogue row's location columns through the
    /// shared organisation catalogue move (`reverse` for Undo). The source
    /// is re-hashed immediately before the swap.
    CommitCatalogue { reverse: bool },
    /// Unlinks a duplicate inside a private directory, only while a verified
    /// copy of the same content is in place at `keeper`.
    DiscardDuplicate {
        path: PathBuf,
        keeper: PathBuf,
        root: PathBuf,
    },
    /// Removes a private directory this plan created, only while empty.
    RemovePrivateDir { dir: PathBuf, root: PathBuf },
    /// Independent end-state verification before Complete may be written.
    FinalVerify,
}

/// Which recorded identity a moved object must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    /// The original source file (same inode and content proof).
    Original,
    /// The staged copy recorded when CopyToStaging completed.
    Staged,
    /// The published destination recorded by the forward transaction
    /// (used by Undo).
    Published,
}

impl Step {
    pub fn label(&self) -> &'static str {
        match self {
            Step::CheckPreconditions => "Check preconditions",
            Step::CreatePrivateDir { .. } => "Create private directory",
            Step::CopyToStaging { .. } => "Copy to staging",
            Step::VerifyStaged { .. } => "Verify staged copy",
            Step::Rename { .. } => "Rename",
            Step::CommitCatalogue { .. } => "Commit catalogue",
            Step::DiscardDuplicate { .. } => "Discard verified duplicate",
            Step::RemovePrivateDir { .. } => "Remove private directory",
            Step::FinalVerify => "Final verification",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanKind {
    Relocate,
    /// Reverses a Complete relocation.
    Undo {
        of_txn: String,
        of_plan_digest: String,
        of_complete_sum: String,
    },
}

/// An immutable plan. Its serialised bytes are hashed into the journal chain;
/// apply refuses a plan whose evidence no longer matches the world.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub format: String,
    pub version: u32,
    pub engine: String,
    pub txn_id: String,
    pub kind: PlanKind,
    pub created_at_unix: u64,
    /// The catalogue database this plan commits to (canonical path).
    pub catalogue: PathBuf,
    /// The shared organisation catalogue binding (database identity, archive
    /// id, old and new associations, projections), captured at planning time
    /// and never rebuilt from the live catalogue.
    pub catalogue_binding: serde_json::Value,
    pub source_root: RootBinding,
    pub destination_root: RootBinding,
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    /// Content-addressed proof of the source captured at planning time
    /// (size, full-precision mtime, inode/device and SHA-256).
    pub source_identity: ObjectIdentity,
    pub strategy: Strategy,
    pub steps: Vec<Step>,
    /// Once the step at this index is Done the engine only moves forward:
    /// earlier failures are compensated, later ones are reported, never
    /// "rolled back" by guessing.
    pub point_of_no_return: usize,
    pub retention: RetentionPolicy,
    /// Digest of the dependency verdict the plan was admitted with.
    pub dependency_digest: String,
    /// Effects the user was shown; they are part of the bound plan.
    pub effects: Vec<String>,
    /// The supported filesystems of the source root, the destination root
    /// and the catalogue's folder when planned (`fscompat`); every step
    /// requires the same type and options.
    pub filesystems: Vec<super::fscompat::MountFacts>,
    /// The filesystem policy the plan was admitted under; every step
    /// re-derives it and refuses a change.
    pub filesystem_policy: super::fscompat::FsPolicy,
    /// For Undo plans: the destination identity the forward transaction
    /// published, which must still be in place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_identity: Option<ObjectIdentity>,
    /// For Undo plans: the retired original's location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Forward,
    Compensate,
}

/// Evidence a step leaves in its Done record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "evidence", rename_all = "snake_case")]
pub enum StepEvidence {
    None,
    Directory {
        id: DirId,
    },
    Identity {
        identity: ObjectIdentity,
    },
    Catalogue {
        rows_changed: u64,
    },
    /// The step was found already applied (or already compensated) by
    /// probing, so no new mutation was made.
    AlreadyDone {
        finding: String,
    },
    Verified {
        summary: String,
    },
    /// Compensation kept an object instead of removing it (it could not be
    /// proven to be a disposable duplicate). Never user content removed.
    Retained {
        path: PathBuf,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// Every forward step is Done and final verification passed.
    Complete,
    /// Nothing the plan changed remains changed; `residue` lists anything
    /// retained instead of removed (never user content).
    Compensated { residue: Vec<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Begun {
        pid: u32,
        engine: String,
    },
    Intent {
        step: usize,
        direction: Direction,
    },
    Done {
        step: usize,
        direction: Direction,
        evidence: StepEvidence,
    },
    /// A step failed without mutating (or its effect was probed absent).
    Failed {
        step: usize,
        direction: Direction,
        error: String,
    },
    RecoveryStarted {
        attempt: u32,
        pid: u32,
        torn_tail_bytes: Option<String>,
    },
    CancelRequested,
    /// The engine stopped without a terminal outcome because it cannot prove
    /// the next transition safe. Recovery may be retried after review; it
    /// re-probes everything. `safe_facts` lists what is known to hold.
    Blocked {
        step: usize,
        reason: String,
        safe_facts: Vec<String>,
    },
    Outcome {
        outcome: Outcome,
    },
    /// Appended to a Complete transaction when its Undo completes.
    UndoneBy {
        undo_txn: String,
    },
}
