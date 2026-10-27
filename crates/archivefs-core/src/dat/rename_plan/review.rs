//! Read-only batch review over the existing audit and rename planner.
//! No approval, transaction, persistence or file mutation is possible here.
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use sha2::{Digest, Sha256};

use super::{ProposalState, RenamePlanContext, RenameProposal, build_rename_plan};
use crate::dat::audit::AuditVerdict;
use crate::dat::classification::multidisc_group_key;
use crate::dat::sources::audit_run::DatAuditOutcome;
use crate::identity_source::hashing::FileFingerprint;
use crate::safe_read::{TrustedRoots, open_bounded_read};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDisposition {
    Eligible,
    Held,
    Refused,
    AlreadyCanonical,
}
impl ReviewDisposition {
    pub fn label(self) -> &'static str {
        match self {
            Self::Eligible => "Eligible for review",
            Self::Held => "Held",
            Self::Refused => "Refused",
            Self::AlreadyCanonical => "Already canonical",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReviewRow {
    pub path: String,
    pub filename: String,
    pub verdict: AuditVerdict,
    pub proposal: Option<RenameProposal>,
    pub disposition: ReviewDisposition,
    pub reasons: Vec<String>,
    pub identity: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CanonicalReview {
    pub source: String,
    pub dat_path: String,
    pub version: Option<String>,
    pub catalogue_sha256: Option<String>,
    pub catalogue_problem: Option<String>,
    pub truncated: bool,
    pub rows: Vec<ReviewRow>,
}

/// Compare current catalogue bytes with the exact snapshot consumed by the
/// audit. A version string or source label is never enough to pass this check.
fn catalogue_problem(outcome: &DatAuditOutcome) -> Option<String> {
    let check = || -> Result<(), String> {
        let expected = outcome
            .catalogue_sha256
            .as_ref()
            .ok_or("No single catalogue digest: use an installed single-file catalogue")?;
        let path = Path::new(&outcome.dat_path);
        let before = FileFingerprint::observe(path).ok_or("Catalogue is unavailable")?;
        let safe = open_bounded_read(path, &TrustedRoots::none()).map_err(|e| e.to_string())?;
        let max = crate::dat::limits::DEFAULT_MAX_FILE_SIZE;
        if before.size_bytes > max {
            return Err("Catalogue exceeds preview read limit".into());
        }
        let mut input = safe.into_file().take(max + 1);
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut bytes = 0u64;
        loop {
            let read = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            bytes += read as u64;
            digest.update(&buffer[..read]);
        }
        if bytes > max || !before.still_current() || format!("{:x}", digest.finalize()) != *expected
        {
            return Err(
                "Stale catalogue evidence: bytes changed since the audit; run a fresh preview"
                    .into(),
            );
        }
        Ok(())
    };
    check().err()
}

/// Reuse canonical naming/collision policy, then add conservative review holds.
/// This is a snapshot, never a capability that the Apply engine can consume.
pub fn build_review(
    outcome: &DatAuditOutcome,
    cancel: &AtomicBool,
) -> Result<CanonicalReview, String> {
    let plan = build_rename_plan(outcome, &RenamePlanContext { generation: 0 }, cancel)
        .map_err(|e| e.to_string())?;
    let catalogue_problem = catalogue_problem(outcome);
    let mut proposals: HashMap<_, _> = plan
        .proposals
        .into_iter()
        .map(|p| (p.source_path.to_string_lossy().into_owned(), p))
        .collect();
    let mut duplicates: BTreeMap<(String, String), usize> = BTreeMap::new();
    for hashes in outcome.known_hashes.values() {
        let key = if let Some(value) = &hashes.sha256 {
            Some(("SHA-256".into(), value.clone()))
        } else if let Some(value) = &hashes.sha1 {
            Some(("SHA-1".into(), value.clone()))
        } else {
            hashes.md5.as_ref().map(|v| ("MD5".into(), v.clone()))
        };
        if let Some(key) = key {
            *duplicates.entry(key).or_default() += 1;
        }
    }
    let mut rows = Vec::new();
    for entry in &outcome.report.entries {
        let proposal = proposals.remove(&entry.local_path);
        let mut row = ReviewRow {
            path: entry.local_path.clone(),
            filename: entry.local_filename.clone(),
            verdict: entry.verdict.clone(),
            proposal,
            disposition: ReviewDisposition::Refused,
            reasons: Vec::new(),
            identity: Vec::new(),
        };
        if let Some(p) = &row.proposal {
            row.disposition = match p.state {
                ProposalState::Suggested => ReviewDisposition::Eligible,
                ProposalState::AlreadyCanonical => ReviewDisposition::AlreadyCanonical,
                ProposalState::Ambiguous
                | ProposalState::Conflict
                | ProposalState::UnclassifiedContent => ReviewDisposition::Held,
                _ => ReviewDisposition::Refused,
            };
            row.reasons.extend(p.blockers.clone());
            row.reasons.extend(p.ambiguity_reason.clone());
            row.reasons
                .extend(p.collision.as_ref().map(|c| c.detail.clone()));
            row.reasons.extend(p.explanations.clone());
            if p.is_outer_archive {
                hold(
                    &mut row,
                    "Archive naming evidence requires a separate set-aware review; no loose-file identity is claimed",
                );
            }
        } else {
            row.reasons.push(match &entry.verdict {
                AuditVerdict::Ambiguous { detail } => format!("Conflicting DAT evidence: {detail}"),
                AuditVerdict::ExactMultipleCandidates { .. } => "Multiple DAT records; no authoritative winner".into(),
                _ => "No supported cryptographic DAT naming proposal; filename and CRC similarity are insufficient".into(),
            });
            if matches!(
                entry.verdict,
                AuditVerdict::Ambiguous { .. } | AuditVerdict::ExactMultipleCandidates { .. }
            ) {
                row.disposition = ReviewDisposition::Held;
            }
        }
        if let Some(hashes) = outcome.known_hashes.get(&entry.local_path) {
            for (algorithm, value) in [
                ("SHA-256", &hashes.sha256),
                ("SHA-1", &hashes.sha1),
                ("MD5", &hashes.md5),
            ] {
                if let Some(value) = value {
                    row.identity.push(format!("{algorithm}: {value}"));
                    if duplicates
                        .get(&(algorithm.into(), value.clone()))
                        .copied()
                        .unwrap_or(0)
                        > 1
                    {
                        hold(
                            &mut row,
                            "Duplicate content identity in this batch; keep every original and review copies together",
                        );
                    }
                }
            }
            if !hashes
                .fingerprint
                .as_ref()
                .is_some_and(FileFingerprint::still_current)
            {
                hold(
                    &mut row,
                    "Stale or unavailable file fingerprint; run a fresh preview",
                );
            }
        } else if row.proposal.is_some() {
            hold(
                &mut row,
                "No current loose-file hash fingerprint; identity cannot be revalidated for this review",
            );
        }
        if let Some(problem) = &catalogue_problem {
            hold(&mut row, problem);
        }
        if outcome.platform.is_none() {
            hold(&mut row, "Catalogue has no assigned platform authority");
        }
        if outcome.truncated {
            hold(
                &mut row,
                "Partial audit: duplicate and relationship coverage is incomplete",
            );
        }
        let game = row.proposal.as_ref().and_then(|p| p.game_name.as_deref());
        let multipart = game.and_then(multidisc_group_key);
        let optical = Path::new(&row.filename)
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| {
                [
                    "cue", "bin", "iso", "chd", "gdi", "m3u", "ccd", "mds", "cso", "pbp",
                ]
                .iter()
                .any(|e| ext.eq_ignore_ascii_case(e))
            });
        // A title token is a warning about related media, never identity proof.
        if let Some(group) = multipart {
            hold(
                &mut row,
                &format!(
                    "Related media: {} (part {} of {}); atomic group/playlist rename is not supported",
                    group.base_title, group.part, group.total
                ),
            );
        } else if optical {
            hold(
                &mut row,
                "Disc/related-media format: companion and playlist rename is not supported; retain the complete set",
            );
        }
        if row.disposition == ReviewDisposition::Eligible {
            check_live_destination(&mut row);
        }
        rows.push(row);
    }
    // Archive planners may emit an outer container absent from the flat audit.
    for (_, proposal) in proposals {
        rows.push(ReviewRow { path: proposal.source_path.to_string_lossy().into_owned(), filename: proposal.current_basename.clone(), verdict: AuditVerdict::NoUsableEvidence, proposal: Some(proposal), disposition: ReviewDisposition::Held, reasons: vec!["Archive set proposal: separate set-aware identity and relationship review required".into()], identity: Vec::new() });
    }
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(CanonicalReview {
        source: outcome.source_display_name.clone(),
        dat_path: outcome.dat_path.clone(),
        version: outcome.catalogue_version.clone(),
        catalogue_sha256: outcome.catalogue_sha256.clone(),
        catalogue_problem,
        truncated: outcome.truncated,
        rows,
    })
}

fn hold(row: &mut ReviewRow, reason: &str) {
    if row.disposition != ReviewDisposition::Refused {
        row.disposition = ReviewDisposition::Held;
    }
    if !row.reasons.iter().any(|s| s == reason) {
        row.reasons.push(reason.into());
    }
}

fn check_live_destination(row: &mut ReviewRow) {
    let Some(p) = &row.proposal else {
        return;
    };
    let Some(name) = &p.proposed_basename else {
        return;
    };
    let Some(parent) = p.source_path.parent() else {
        return;
    };
    let mut siblings = super::DirSiblings::default();
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(e) => {
            hold(row, &format!("Cannot check destination folder: {e}"));
            return;
        }
    };
    for (count, entry) in entries.enumerate() {
        if count >= crate::dat::sources::audit_run::MAX_SCAN_ENTRIES_EXAMINED {
            hold(row, "Destination folder exceeds review limit");
            return;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                hold(row, &format!("Cannot check destination entry: {e}"));
                return;
            }
        };
        let Some(sibling) = entry.file_name().to_str().map(str::to_owned) else {
            hold(row, "Destination contains an unrepresentable filename");
            return;
        };
        // More conservative Unicode lowercase check in addition to the existing
        // planner's ASCII collision rules. Never resolves a collision.
        if sibling != p.current_basename && sibling.to_lowercase() == name.to_lowercase() {
            hold(
                row,
                "Destination collision (including case folding); existing files, directories and symlinks are preserved",
            );
            return;
        }
        siblings.names.insert(sibling);
    }
    if let Some(collision) = super::detect_target_collision(&p.current_basename, name, &siblings) {
        hold(row, &collision.detail);
    }
}

#[cfg(test)]
#[path = "review/tests.rs"]
mod tests;
