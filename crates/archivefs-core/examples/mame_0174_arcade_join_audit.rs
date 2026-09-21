use std::path::PathBuf;

use archivefs_core::dat::mame_arcade_join::{join_extracted_arcade_root, load_verified_mame_0174};

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let dat_path = arguments.next().map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("usage: mame_0174_arcade_join_audit <mame-0.174-dat> <arcade-root>");
        std::process::exit(2);
    });
    let root = arguments.next().map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("usage: mame_0174_arcade_join_audit <mame-0.174-dat> <arcade-root>");
        std::process::exit(2);
    });
    if arguments.next().is_some() {
        eprintln!("usage: mame_0174_arcade_join_audit <mame-0.174-dat> <arcade-root>");
        std::process::exit(2);
    }
    let dat = load_verified_mame_0174(&dat_path)
        .expect("the specified MAME 0.174 DAT must verify");
    let audited_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_secs()
        .to_string();
    let report = join_extracted_arcade_root(&dat, &root, &audited_at)
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
