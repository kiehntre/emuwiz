//! Shared, read-only Saturn patch admission. Composes the canonical optical
//! inspector and exact manifest verification; never claims disc equivalence.
use crate::mame_collection_analyser::inspection::SourceStamp;
use crate::optical_layout_inspection::{
    CompanionStatus, OpticalLayoutState, inspect_optical_layout,
};
use crate::optical_patch_tree::refuse;
use crate::saturn_disc_manifest::{
    SaturnDescriptorType, SaturnDiscManifest, verify_saturn_manifest,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaturnOpticalPreflight {
    pub basis: String,
    pub limits: Vec<String>,
    /// Empty means this source passed admission, never that a patch is applicable.
    pub refusals: Vec<String>,
}

pub fn assess(manifest: &SaturnDiscManifest) -> SaturnOpticalPreflight {
    let mut result = SaturnOpticalPreflight {
        basis: "Verification method: canonical CUE/BIN structure and timeline, plus exact descriptor, component and audio hashes compared with the reviewed manifest.".into(),
        limits: vec!["Source evidence only: no patch output, full disc equivalence, DAT match or playability is established.".into()],
        refusals: Vec::new(),
    };
    if manifest.descriptor_type != SaturnDescriptorType::Cue {
        result.refusals.push("Patch layout requires a complete CUE/BIN set; lone BIN evidence cannot establish track or audio layout.".into());
        return result;
    }
    // Refuse nonregular/replaced descriptors before any canonical parser opens
    // them: a FIFO must never turn a readiness check into a blocking read.
    let descriptor_stamp = match SourceStamp::observe(&manifest.source_descriptor) {
        Ok(stamp) if !stamp.directory => stamp,
        _ => {
            result.refusals.push("Reviewed descriptor is missing or unsafe; a regular CUE file is required before optical preflight.".into());
            return result;
        }
    };
    if let Ok(paths) = crate::ingestion::cue_bin::resolve_cue_all_files(&manifest.source_descriptor)
    {
        for path in paths {
            if !manifest.components.iter().any(|c| c.path == path) {
                result.refusals.push(format!(
                    "Referenced companion is absent from the reviewed manifest: {}",
                    path.display()
                ));
            }
        }
    }
    let stamps = manifest
        .components
        .iter()
        .map(|c| SourceStamp::observe(&c.path))
        .collect::<Result<Vec<_>, _>>();
    match inspect_optical_layout(&manifest.source_descriptor) {
        Some(report) => {
            if report.state != OpticalLayoutState::Valid {
                result.refusals.push(format!(
                    "Optical layout {:?}: {}",
                    report.state, report.summary
                ));
            }
            for row in report.companions {
                match row.status {
                    CompanionStatus::Present { .. } => {}
                    status => result
                        .refusals
                        .push(format!("Companion {}: {status:?}", row.name)),
                }
            }
        }
        None => result
            .refusals
            .push("Descriptor representation is not covered by the CUE/BIN patch verifier.".into()),
    }
    if let Err(error) = admit_cue(&manifest.source_descriptor) {
        result.refusals.push(error.to_string());
    }
    let verification = verify_saturn_manifest(manifest);
    for issue in verification.issues {
        result
            .refusals
            .push(format!("Reviewed source changed: {issue:?}"));
    }
    if !descriptor_stamp.still_matches()
        || !stamps.is_ok_and(|stamps| stamps.iter().all(SourceStamp::still_matches))
    {
        result.refusals.push("Descriptor or companion changed during optical preflight, is missing, or is unsafe; inspect again.".into());
    }
    result
}

// Admission only, not a second CUE parser: the canonical parser below owns
// paths, tracks and timestamps. Refuse directives/FILE types it drops rather
// than allowing an incomplete projection to authorize a component write.
fn admit_cue(path: &Path) -> io::Result<()> {
    let mut text = String::new();
    fs::File::open(path)?
        .take(crate::ingestion::cue_bin::MAX_CUE_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > crate::ingestion::cue_bin::MAX_CUE_BYTES {
        return Err(refuse("CUE size bound"));
    }
    for raw in text.lines() {
        let line = raw.trim_matches([' ', '\t', '\r']);
        if line.is_empty() {
            continue;
        }
        if line.chars().any(|c| c.is_control() && c != '\t') {
            return Err(refuse("CUE control character"));
        }
        let mut fields = line.split_ascii_whitespace();
        let directive = fields
            .next()
            .ok_or_else(|| refuse("missing CUE directive"))?;
        let rest = line[directive.len()..].trim_matches([' ', '\t']);
        let supported = match directive.to_ascii_uppercase().as_str() {
            "FILE" => rest
                .strip_prefix('"')
                .and_then(|s| s.split_once('"'))
                .is_some_and(|(name, kind)| {
                    !name.is_empty()
                        && !name.contains('\\')
                        && kind.trim().eq_ignore_ascii_case("BINARY")
                }),
            "TRACK" => {
                fields
                    .next()
                    .and_then(|v| v.parse::<u32>().ok())
                    .is_some_and(|n| (1..=99).contains(&n))
                    && fields.next().is_some_and(|v| {
                        matches!(
                            v.to_ascii_uppercase().as_str(),
                            "MODE1/2048" | "MODE1/2352" | "MODE2/2352" | "AUDIO"
                        )
                    })
                    && fields.next().is_none()
            }
            "INDEX" => {
                fields
                    .next()
                    .and_then(|v| v.parse::<u8>().ok())
                    .is_some_and(|n| n <= 1)
                    && fields.next().is_some()
                    && fields.next().is_none()
            }
            "PREGAP" | "POSTGAP" => fields.next().is_some() && fields.next().is_none(),
            "REM" => fields.next().is_some_and(|v| {
                v.eq_ignore_ascii_case("COMMENT") || v.eq_ignore_ascii_case("GENRE")
            }),
            "TITLE" => {
                rest.len() >= 2
                    && rest.starts_with('"')
                    && rest.ends_with('"')
                    && !rest[1..rest.len() - 1].contains('"')
            }
            _ => false,
        };
        if !supported {
            return Err(refuse(format!(
                "CUE directive {directive} is outside the verified component-patch layout (only INDEX 00/01 and explicitly supported directives are admitted)"
            )));
        }
    }
    Ok(())
}
