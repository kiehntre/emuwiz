use std::path::Path;

use archivefs_core::Database;
use archivefs_core::dat::mame_arcade_join::{join_extracted_arcade_root, load_verified_mame_0174};
use archivefs_core::default_database_path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dat_path = Path::new("/home/davedap/DATs/MAME/MAME.0.174.Arcade.XML.dat");
    let root = Path::new("/mnt/usbdrive/games/arcade");
    let dat = load_verified_mame_0174(dat_path)?;
    let report = join_extracted_arcade_root(&dat, root, "production-member-location-proof")?;
    let database_path = default_database_path()?;
    let mut database = Database::open_or_create(&database_path)?;
    let rows = database.persist_mame_arcade_join(&report)?;
    println!("persisted={rows} members={}", report.evidence.len());
    Ok(())
}
