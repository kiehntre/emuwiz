use std::path::Path;

use archivefs_core::dat::mame_arcade_join::{join_extracted_arcade_root, load_verified_mame_0174};

fn main() {
    let dat_path = Path::new("/home/davedap/EmuWiz/DATs/MAME/MAME.0.174.Arcade.XML.dat");
    let root = Path::new("/mnt/usbdrive/games/arcade");
    let dat = load_verified_mame_0174(dat_path).expect("the specified MAME 0.174 DAT must verify");
    let audited_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_secs()
        .to_string();
    let report = join_extracted_arcade_root(&dat, root, &audited_at)
        .expect("the proven Arcade root must be readable");
    println!(
        "DAT {} {} machines",
        report.dat_sha256, report.dat_machine_count
    );
    println!("SUMMARY {:?}", report.summary);
    println!("LAYOUT {}", report.layout_estimate);
    for name in ["pacman", "donpachi", "1942"] {
        if let Some(item) = report
            .evidence
            .iter()
            .find(|item| item.logical_set_name == name)
        {
            println!(
                "REPRESENTATIVE {name} {}",
                serde_json::to_string(item).unwrap()
            );
        }
    }
}
