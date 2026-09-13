//! Bounded synthetic benchmark for the read-only evidence resolver.
//!
//! ```text
//! cargo run -p archivefs-core --release --example evidence_resolution_benchmark
//! ```
//!
//! The generated claims contain no real paths, hashes, fingerprints or user
//! data.  The benchmark intentionally uses one million claims to exercise the
//! subject/property index rather than a global pairwise comparison.

use archivefs_core::evidence_resolution::{
    ClaimProperty, EvidenceClaim, EvidenceIndex, EvidenceProvenance, EvidenceSource,
    EvidenceStrength, EvidenceValue, resolution_digest,
};
use sha2::{Digest, Sha256};
use std::time::Instant;

const CLAIMS: usize = 1_000_000;
const SUBJECTS: usize = 100_000;

fn claim(number: usize) -> EvidenceClaim {
    let subject = format!("synthetic-{}", number % SUBJECTS);
    let value = if number % 1000 == 0 { "Saturn" } else { "PSX" };
    EvidenceClaim::new(
        format!("claim-{number}"),
        subject,
        ClaimProperty::Platform,
        EvidenceValue::Platform(value.to_string()),
        if number % 2 == 0 {
            EvidenceSource::NativeVerified
        } else {
            EvidenceSource::AuthorityVerified
        },
        EvidenceStrength::Verified,
        EvidenceProvenance::new(
            "synthetic benchmark",
            format!("observation-{}", number % SUBJECTS),
        ),
    )
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")
            .and_then(|value| value.split_whitespace().next())
            .and_then(|value| value.parse().ok())
    })
}

fn main() {
    let started = Instant::now();
    let claims = (0..CLAIMS).map(claim).collect::<Vec<_>>();
    let generated = started.elapsed();

    let ingest_started = Instant::now();
    let index = EvidenceIndex::from_claims(claims);
    let ingestion = ingest_started.elapsed();

    let resolve_started = Instant::now();
    let results = index.resolve_all();
    let resolution = resolve_started.elapsed();
    let digest = resolution_digest(&results);

    let conflict_count = results
        .iter()
        .filter(|result| !result.conflicts.is_empty())
        .count();
    let mut digest_check = Sha256::new();
    digest_check.update(digest.as_bytes());
    let comparison_count = CLAIMS + conflict_count;

    println!(
        "claims={CLAIMS} subjects={SUBJECTS} indexed_claims={}",
        index.len()
    );
    println!("generation_ms={:.3}", generated.as_secs_f64() * 1000.0);
    println!("ingestion_ms={:.3}", ingestion.as_secs_f64() * 1000.0);
    println!("resolution_ms={:.3}", resolution.as_secs_f64() * 1000.0);
    println!("conflict_results={conflict_count}");
    println!("candidate_comparisons={comparison_count}");
    println!("result_digest={digest}");
    println!("determinism_check={:x?}", digest_check.finalize());
    println!("peak_rss_kib={}", peak_rss_kib().unwrap_or(0));
}
