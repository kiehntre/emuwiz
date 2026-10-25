//! Read-only local catalogue discovery; prints evidence-bearing records as JSON.

use archivefs_core::homebrew_artifact::ProjectDiscoveryProvider;
use archivefs_core::homebrew_catalogue::LocalCatalogueProvider;
use std::fmt::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 3 {
        return Err(
            "usage: homebrew_discover <local-catalogue.json> [query] [canonical-platform-id]"
                .into(),
        );
    }
    let provider = LocalCatalogueProvider::from_path(&args[0])?;
    for claim in provider.unregistered_platform_claims() {
        // Escape each untrusted string for display without changing its identity.
        eprintln!(
            "note: {:?}/{:?} build {:?} claims unregistered platform {:?}; it cannot match a platform filter",
            claim.project.provider.as_str(),
            claim.project.provider_id,
            claim.provider_asset_id,
            claim.platform
        );
    }
    let query = args.get(1).map(String::as_str).unwrap_or("");
    let projects = match args.get(2) {
        Some(platform) => provider.discover_for_platform(query, platform)?,
        None => provider.discover_projects(query)?,
    };
    let records = projects
        .iter()
        .map(|project| provider.record(&project.id))
        .collect::<Result<Vec<_>, _>>()?;
    let json = serde_json::to_string_pretty(&records)?;
    // JSON permits literal DEL/C1 controls. Escape them for terminal output using
    // JSON's Unicode syntax so decoded records retain their exact values.
    let mut output = String::with_capacity(json.len());
    for c in json.chars() {
        if matches!(c, '\u{7f}'..='\u{9f}') {
            write!(output, "\\u{:04x}", c as u32)?;
        } else {
            output.push(c);
        }
    }
    println!("{output}");
    Ok(())
}
