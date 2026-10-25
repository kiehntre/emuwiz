//! Ad hoc, read-only real-platform-mapping audit for Publisher Profiles
//! Phase 1 (task section 21). Not a `cargo test` because it is a one-off
//! research artifact, not a regression check; its output is captured
//! verbatim into `docs/research/PUBLISHER_PROFILES_PHASE1.md`.
//!
//! This performs no filesystem scan of any real library and no source or
//! destination mutation: it only calls this crate's own existing,
//! reviewed platform-mapping tables for a representative set of platforms
//! actually present in this machine's real ROM collection (confirmed via
//! `ls /mnt` directory names, not fabricated).

use archivefs_core::launch::es_de_export::es_de_system_for_platform;
use archivefs_core::platform_evidence_fusion::romm_platform_mapping::{
    FrontendPlatformMapping, production_romm_status,
};
use archivefs_core::publisher_profile::{PublisherPlatformMapping, es_de, romm};

fn main() {
    // Representative platforms this machine's real library actually has
    // folders for (`/mnt/ROM`, `/mnt/saturn-roms`, `/mnt/x68000-roms`,
    // `/mnt/gba-roms`, `/mnt/x32-roms`, plus TOSEC/No-Intro DAT packs under
    // `/mnt/DATs` covering PS1/PS2/Amiga/Atari ST/Spectrum).
    let platforms = [
        "PSX",
        "PS2",
        "Amiga",
        "AtariST",
        "ZX Spectrum",
        "Dreamcast",
        "Saturn",
        "Sharp X68000",
        "Game Boy Advance",
    ];

    println!("frontend,canonical_platform_id,status,folder");
    let overrides = FrontendPlatformMapping::default();
    for platform in platforms {
        let romm_mapping = romm::resolve_romm_platform_mapping(platform, &overrides, None);
        print_row("RomM", &romm_mapping);

        let es_de_mapping = es_de::resolve_es_de_platform_mapping(platform);
        print_row("ES-DE", &es_de_mapping);
    }

    // Cross-check against the direct table functions too, so this audit's
    // own `romm`/`es_de` wrapper functions are shown agreeing with the
    // underlying production functions they delegate to - no double
    // standard between "what Publisher Profile reports" and "what
    // production RomM/ES-DE integration already reports".
    println!();
    println!("cross-check against underlying production functions:");
    for platform in platforms {
        let status = production_romm_status(platform, &overrides, None);
        let es_de = es_de_system_for_platform(platform);
        println!(
            "{platform}: production_romm_status={status:?} es_de_system_for_platform={:?}",
            es_de.map(|m| m.es_de_system)
        );
    }
}

fn print_row(frontend: &str, mapping: &PublisherPlatformMapping) {
    let (status, folder) = match mapping {
        PublisherPlatformMapping::Mapped { folder, .. } => ("Mapped", folder.as_str()),
        PublisherPlatformMapping::Unmapped { .. } => ("Unmapped", "-"),
        PublisherPlatformMapping::Ambiguous { .. } => ("Ambiguous", "-"),
        PublisherPlatformMapping::Unsupported { .. } => ("Unsupported", "-"),
    };
    println!(
        "{frontend},{},{status},{folder}",
        mapping.canonical_platform_id()
    );
}
