//! Read-only orchestration of independent MAME and FBNeo compatibility audits.
//!
//! This is deliberately a projection boundary, not another compatibility
//! engine.  Callers supply one already-observed arcade set and the expectation
//! snapshots for the installed emulators.  The module performs no scanning,
//! archive opening, hashing, process execution, catalogue access, launch
//! selection, or mutation.

use serde::Serialize;

use crate::arcade_fbneo_compatibility::{
    FbNeoSetCompatibility, FbNeoSetCompatibilityState, FbNeoSetExpectation, InstalledFbNeoEvidence,
    audit_fbneo_set,
};
use crate::arcade_mame_compatibility::{
    InstalledMameEvidence, MameSetCompatibility, MameSetCompatibilityState, MameSetExpectation,
    ObservedArcadeSetEvidence,
};

/// Result of evaluating one installed-emulator expectation against the shared
/// observed evidence.  An absent emulator is distinct from an installed
/// emulator for which usable expectations were not gathered.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(
    tag = "status",
    content = "result",
    rename_all = "SCREAMING_SNAKE_CASE"
)]
pub enum ArcadeEmulatorCompatibility<T> {
    NotInstalled,
    Unknown,
    Evaluated(T),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArcadeCompatibilityComparison {
    BothCompatible,
    MameOnly,
    FbneoOnly,
    BothIncompatible,
    InsufficientEvidence,
    EmulatorNotInstalled,
}

/// Independent MAME and FBNeo results for one observed arcade set.
///
/// `comparison` is descriptive only.  It is not a preference, fallback, or
/// launch-policy decision, and it never replaces either result's own
/// `ready_to_play_state` projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArcadeEmulatorCompatibilitySummary {
    pub set_name: String,
    pub mame: ArcadeEmulatorCompatibility<MameSetCompatibility>,
    pub fbneo: ArcadeEmulatorCompatibility<FbNeoSetCompatibility>,
    pub comparison: ArcadeCompatibilityComparison,
}

/// Evaluate one existing observed-evidence projection against both installed
/// emulator expectation snapshots.
///
/// `None` means that the corresponding emulator is not installed/available to
/// this workflow.  `Some((evidence, None))` means the emulator is known but no
/// expectation for this set was gathered; that result remains `Unknown` for
/// MAME and is handled by the existing FBNeo engine for FBNeo.
pub fn evaluate_arcade_compatibility(
    observed: &ObservedArcadeSetEvidence,
    mame: Option<(&InstalledMameEvidence, Option<MameSetExpectation>)>,
    fbneo: Option<(&InstalledFbNeoEvidence, Option<FbNeoSetExpectation>)>,
) -> ArcadeEmulatorCompatibilitySummary {
    let mame_result = match mame {
        None => ArcadeEmulatorCompatibility::NotInstalled,
        Some((evidence, Some(expectation))) => {
            ArcadeEmulatorCompatibility::Evaluated(observed.audit(evidence, expectation))
        }
        Some((_evidence, None)) => ArcadeEmulatorCompatibility::Unknown,
    };
    let fbneo_result = match fbneo {
        None => ArcadeEmulatorCompatibility::NotInstalled,
        Some((evidence, expectation)) => {
            ArcadeEmulatorCompatibility::Evaluated(audit_fbneo_set(evidence, expectation, observed))
        }
    };
    let comparison = comparison_for(&mame_result, &fbneo_result);
    ArcadeEmulatorCompatibilitySummary {
        set_name: observed.set_name.clone(),
        mame: mame_result,
        fbneo: fbneo_result,
        comparison,
    }
}

fn mame_compatible(result: &ArcadeEmulatorCompatibility<MameSetCompatibility>) -> bool {
    matches!(
        result,
        ArcadeEmulatorCompatibility::Evaluated(MameSetCompatibility {
            state: MameSetCompatibilityState::Compatible
                | MameSetCompatibilityState::CompatibleWithWarnings,
            ..
        })
    )
}

fn fbneo_compatible(result: &ArcadeEmulatorCompatibility<FbNeoSetCompatibility>) -> bool {
    matches!(
        result,
        ArcadeEmulatorCompatibility::Evaluated(FbNeoSetCompatibility {
            state: FbNeoSetCompatibilityState::Compatible
                | FbNeoSetCompatibilityState::CompatibleWithWarnings,
            ..
        })
    )
}

fn evaluated(result: &ArcadeEmulatorCompatibility<impl Sized>) -> bool {
    matches!(result, ArcadeEmulatorCompatibility::Evaluated(_))
}

fn comparison_for(
    mame: &ArcadeEmulatorCompatibility<MameSetCompatibility>,
    fbneo: &ArcadeEmulatorCompatibility<FbNeoSetCompatibility>,
) -> ArcadeCompatibilityComparison {
    if matches!(mame, ArcadeEmulatorCompatibility::NotInstalled)
        || matches!(fbneo, ArcadeEmulatorCompatibility::NotInstalled)
    {
        return ArcadeCompatibilityComparison::EmulatorNotInstalled;
    }
    if mame_compatible(mame) && fbneo_compatible(fbneo) {
        ArcadeCompatibilityComparison::BothCompatible
    } else if mame_compatible(mame) {
        ArcadeCompatibilityComparison::MameOnly
    } else if fbneo_compatible(fbneo) {
        ArcadeCompatibilityComparison::FbneoOnly
    } else if evaluated(mame) && evaluated(fbneo) {
        ArcadeCompatibilityComparison::BothIncompatible
    } else {
        ArcadeCompatibilityComparison::InsufficientEvidence
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arcade_mame_compatibility::{
        MameExpectedRom, MameObservedRom, ObservedEvidenceCompleteness,
    };
    use crate::dat::model::{ChecksumAlgorithm, DatChecksum};

    fn observed() -> ObservedArcadeSetEvidence {
        let mut evidence =
            ObservedArcadeSetEvidence::new("neogeo", ObservedEvidenceCompleteness::Complete);
        evidence.observed_roms.push(MameObservedRom {
            set_name: "neogeo".into(),
            name: "bios.bin".into(),
            size_bytes: Some(4),
            crc32: Some("12345678".into()),
            sha1: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
        });
        evidence.observed_set_names.push("neogeo".into());
        evidence
    }

    fn mame() -> InstalledMameEvidence {
        InstalledMameEvidence::new(
            "/usr/games/mame",
            "0.264",
            "/tmp/listxml",
            ["/roms".into()],
            Some("listxml-hash".into()),
            Some(1),
        )
    }

    fn mame_expectation() -> MameSetExpectation {
        MameSetExpectation {
            set_name: "neogeo".into(),
            parent: None,
            rom_of: None,
            runnable: Some("yes".into()),
            is_bios: Some("yes".into()),
            is_device: Some("no".into()),
            driver_status: Some("good".into()),
            roms: vec![MameExpectedRom {
                set_name: "neogeo".into(),
                name: "bios.bin".into(),
                size_bytes: Some(4),
                checksums: vec![DatChecksum {
                    algorithm: ChecksumAlgorithm::Crc32,
                    value: "12345678".into(),
                }],
                status: None,
                merge: None,
            }],
            disks: Vec::new(),
            dependencies: crate::arcade_mame_compatibility::MameDependencyEvidence {
                set_name: "neogeo".into(),
                parent: None,
                rom_of: None,
                device_refs: Vec::new(),
                closure_set_names: vec!["neogeo".into()],
                runnable: Some("yes".into()),
                is_bios: Some("yes".into()),
                is_device: Some("no".into()),
            },
        }
    }

    fn fbneo() -> InstalledFbNeoEvidence {
        InstalledFbNeoEvidence::new(
            "/tmp/fbneo.so",
            Some("1.0".into()),
            None,
            "/tmp/fbneo.dat",
            Some("1".into()),
            Some("dat-hash".into()),
            Some(1),
        )
    }

    #[test]
    fn one_observation_feeds_both_independent_engines() {
        let summary = evaluate_arcade_compatibility(
            &observed(),
            Some((&mame(), Some(mame_expectation()))),
            Some((&fbneo(), None)),
        );
        assert!(matches!(
            summary.mame,
            ArcadeEmulatorCompatibility::Evaluated(_)
        ));
        assert!(matches!(
            summary.fbneo,
            ArcadeEmulatorCompatibility::Evaluated(_)
        ));
        assert_eq!(summary.comparison, ArcadeCompatibilityComparison::MameOnly);
    }

    #[test]
    fn absent_emulator_does_not_hide_other_result() {
        let summary = evaluate_arcade_compatibility(
            &observed(),
            Some((&mame(), Some(mame_expectation()))),
            None,
        );
        assert!(mame_compatible(&summary.mame));
        assert_eq!(
            summary.comparison,
            ArcadeCompatibilityComparison::EmulatorNotInstalled
        );
    }

    #[test]
    fn partial_observation_is_not_promoted_by_orchestration() {
        let mut evidence = observed();
        evidence.completeness = ObservedEvidenceCompleteness::Partial;
        let summary = evaluate_arcade_compatibility(
            &evidence,
            Some((&mame(), Some(mame_expectation()))),
            Some((&fbneo(), None)),
        );
        assert!(matches!(
            summary.mame,
            ArcadeEmulatorCompatibility::Evaluated(MameSetCompatibility {
                state: MameSetCompatibilityState::Unknown,
                ..
            })
        ));
        assert_eq!(
            summary.comparison,
            ArcadeCompatibilityComparison::InsufficientEvidence
        );
    }

    #[test]
    fn mame_and_fbneo_expectations_remain_separate() {
        let mut wrong_mame = mame_expectation();
        wrong_mame.roms[0].name = "other.bin".into();
        let summary = evaluate_arcade_compatibility(
            &observed(),
            Some((&mame(), Some(wrong_mame))),
            Some((&fbneo(), None)),
        );
        assert!(matches!(
            summary.mame,
            ArcadeEmulatorCompatibility::Evaluated(MameSetCompatibility {
                state: MameSetCompatibilityState::Incompatible,
                ..
            })
        ));
        assert!(matches!(
            summary.fbneo,
            ArcadeEmulatorCompatibility::Evaluated(_)
        ));
    }
}
