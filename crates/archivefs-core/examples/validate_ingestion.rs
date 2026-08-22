//! Ad-hoc validation harness for `ingestion::discover_source` against a
//! real collection. Read-only: only calls `discover_source`, never writes
//! anything. Not part of the test suite - this is exploratory tooling for
//! the Universal Source Ingestion validation pass.
//!
//! Usage: `cargo run --release --example validate_ingestion -- <path> [--sample N]`

use archivefs_core::ingestion::{ValidationState, discover_source};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("usage: validate_ingestion <path>"));
    let mut sample = 20usize;
    let mut extra_args: Vec<String> = args.collect();
    if let Some(index) = extra_args.iter().position(|a| a == "--sample") {
        sample = extra_args
            .get(index + 1)
            .and_then(|value| value.parse().ok())
            .unwrap_or(20);
        extra_args.drain(index..(index + 2).min(extra_args.len()));
    }

    println!("Discovering: {}", path.display());
    let start = Instant::now();
    let report = discover_source(&path).expect("discover_source failed");
    let elapsed = start.elapsed();

    println!("\n=== Timing ===");
    println!("Elapsed: {:.2?}", elapsed);
    println!("Items: {}", report.items.len());
    if !report.items.is_empty() {
        println!(
            "Per-item: {:.1}us",
            elapsed.as_micros() as f64 / report.items.len() as f64
        );
    }

    println!("\n=== Stats ===");
    println!("{:#?}", report.stats);

    let accepted = report
        .items
        .iter()
        .filter(|i| i.validation_state == ValidationState::Accepted)
        .count();
    let skipped = report.items.len() - accepted;
    println!("\nAccepted: {accepted}  Skipped: {skipped}");

    println!("\n=== Skip reason breakdown ===");
    let mut by_reason: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for item in &report.items {
        if let Some(reason) = &item.skip_reason {
            *by_reason.entry(reason.label()).or_default() += 1;
        }
    }
    for (label, count) in &by_reason {
        println!("{label}: {count}");
    }

    println!("\n=== Sample skipped items (up to {sample}) ===");
    for item in report
        .items
        .iter()
        .filter(|i| i.skip_reason.is_some())
        .take(sample)
    {
        println!(
            "- {}\n    reason: {}\n    explanation: {}\n    suggested action: {}",
            item.path.display(),
            item.skip_reason.as_ref().unwrap().label(),
            item.explanation,
            item.skip_reason.as_ref().unwrap().suggested_action(),
        );
    }

    println!("\n=== Sample accepted items (up to {sample}) ===");
    for item in report
        .items
        .iter()
        .filter(|i| i.validation_state == ValidationState::Accepted)
        .take(sample)
    {
        println!(
            "- {}\n    content: {:?}  platform: {:?}\n    explanation: {}",
            item.path.display(),
            item.content,
            item.platform_hint,
            item.explanation,
        );
    }
}
