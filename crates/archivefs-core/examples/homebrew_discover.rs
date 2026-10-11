//! Read-only local catalogue discovery; prints evidence-bearing records as JSON.

use archivefs_core::homebrew_artifact::ProjectDiscoveryProvider;
use archivefs_core::homebrew_catalogue::LocalCatalogueProvider;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 3 {
        return Err(
            "usage: homebrew_discover <local-catalogue.json> [query] [canonical-platform-id]"
                .into(),
        );
    }
    let provider = LocalCatalogueProvider::from_path(&args[0])?;
    let query = args.get(1).map(String::as_str).unwrap_or("");
    let projects = match args.get(2) {
        Some(platform) => provider.discover_for_platform(query, platform)?,
        None => provider.discover_projects(query)?,
    };
    let records = projects
        .iter()
        .map(|project| provider.record(&project.id))
        .collect::<Result<Vec<_>, _>>()?;
    println!("{}", serde_json::to_string_pretty(&records)?);
    Ok(())
}
