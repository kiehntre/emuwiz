//! 100,000-item Publisher Profile planning benchmark - task section 24.
//!
//! Synthetic (no real filesystem I/O): builds 100,000 distinct
//! [`ElectedGame`] entries in-memory and times `build_publisher_plan`
//! end-to-end, plus reads this process's own peak RSS from
//! `/proc/self/status` (Linux-only; falls back to "unavailable"
//! elsewhere).

use std::path::PathBuf;
use std::time::Instant;

use archivefs_core::platform_evidence_fusion::romm_platform_mapping::FrontendPlatformMapping;
use archivefs_core::playing_library::{
    CandidateEvidenceSummary, ElectedGame, ElectionExplanation, LinkedLibraryOperation,
    PlayingLibraryPlan, PlayingLibraryPolicy,
};
use archivefs_core::publisher_profile::planner::{PublisherPlanRequest, build_publisher_plan};
use archivefs_core::publisher_profile::romm::{resolve_romm_platform_mapping, romm_profile};

const ITEM_COUNT: usize = 100_000;

fn synthetic_plan(count: usize) -> PlayingLibraryPlan {
    let elected_games: Vec<ElectedGame> = (0..count)
        .map(|index| ElectedGame {
            dat_entry_name: format!("Synthetic Game {index:06} (USA)"),
            family_root_name: format!("Synthetic Game {index:06} (USA)"),
            explanation: ElectionExplanation {
                steps: vec!["the only election-eligible release in its family".to_string()],
                rejected: Vec::new(),
                winner_evidence: CandidateEvidenceSummary::unknown(),
            },
            launcher_operation: LinkedLibraryOperation {
                source_path: PathBuf::from(format!("/source/synthetic-game-{index:06}.rom")),
                destination_path: PathBuf::from(format!(
                    "/source-library/synthetic-game-{index:06}.rom"
                )),
            },
            companion_operations: Vec::new(),
        })
        .collect();
    PlayingLibraryPlan {
        destination_root: PathBuf::from("/source-library"),
        policy: PlayingLibraryPolicy::default(),
        archives_examined: count,
        families_examined: count,
        elected_games,
        unresolved_groups: Vec::new(),
        exclusions: Vec::new(),
        singleton_families: count,
        conflicts: Vec::new(),
        operations: Vec::new(),
        rejected_launchers: Vec::new(),
    }
}

fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmHWM:") {
            return value.trim().trim_end_matches(" kB").trim().parse().ok();
        }
    }
    None
}

fn main() {
    println!("Publisher Profile Phase 1 benchmark: {ITEM_COUNT} items");

    let build_input_started = Instant::now();
    let plan = synthetic_plan(ITEM_COUNT);
    println!(
        "  synthetic PlayingLibraryPlan construction: {:?}",
        build_input_started.elapsed()
    );

    let mapping_started = Instant::now();
    let mapping = resolve_romm_platform_mapping("Amiga", &FrontendPlatformMapping::default(), None);
    println!(
        "  profile platform mapping resolution: {:?}",
        mapping_started.elapsed()
    );

    let plan_started = Instant::now();
    let request = PublisherPlanRequest {
        profile: &romm_profile(),
        playing_library_plan: &plan,
        platform_mapping: mapping,
        destination_root: PathBuf::from("/library"),
        existing_destination_root: None,
    };
    let result = build_publisher_plan(&request).expect("synthetic plan must succeed");
    let plan_elapsed = plan_started.elapsed();
    println!("  destination planning + conflict detection: {plan_elapsed:?}");

    let summary_started = Instant::now();
    let summary = result.summary();
    println!("  summary computation: {:?}", summary_started.elapsed());

    println!("  total items: {}", result.items.len());
    println!("  will_publish: {}", summary.will_publish);
    println!("  plan_hash: {}", result.plan_hash);
    match peak_rss_kb() {
        Some(kb) => println!("  peak RSS: {kb} kB ({:.1} MB)", kb as f64 / 1024.0),
        None => println!("  peak RSS: unavailable on this platform"),
    }
    println!(
        "  total wall time (excluding synthetic input construction): {:?}",
        plan_elapsed
    );
}
