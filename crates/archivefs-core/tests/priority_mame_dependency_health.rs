//! Historical 6f7bceee bypass fixtures adapted to #159's typed evidence API.
//! CHD header identity is observed but cannot be passed as verified content.
//! Unique support ROM names keep missing/corrupt fixtures distinct from a valid
//! non-merged host copy, which the current analyser legitimately accepts.
use archivefs_core::dat::archive::chd::read_chd_v5_header;
use archivefs_core::dat::model::{DatEcosystem, ParsedDat};
use archivefs_core::dat::parsers::parse_dat_file;
use archivefs_core::mame_collection_analyser::*;
use archivefs_core::mame_collection_analyser::{
    MameObservedMember, MameObservedSet, analyse_collection,
};
use archivefs_core::mame_playing_library::*;
use sha1::{Digest, Sha1};
use std::path::PathBuf;

const DISK_SHA1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ROM_BYTES: &[u8] = &[1, 2, 3, 4];

fn rom_sha1() -> String {
    Sha1::digest(ROM_BYTES)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn rom_xml() -> String {
    format!("<rom name=\"rom.bin\" size=\"4\" sha1=\"{}\"/>", rom_sha1())
}

fn disk_xml() -> String {
    format!("<disk name=\"disk\" sha1=\"{DISK_SHA1}\"/>")
}

fn parsed(machines: &str) -> ParsedDat {
    parsed_document(&format!("<mame build=\"0.280\">{machines}</mame>"))
}

fn parsed_document(xml: &str) -> ParsedDat {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("listxml.xml");
    std::fs::write(&path, xml).unwrap();
    let dat = parse_dat_file(&path, Default::default()).unwrap().dat;
    assert_eq!(dat.source.ecosystem, DatEcosystem::MAMEArcade);
    dat
}

fn rom_member() -> MameObservedMember {
    MameObservedMember {
        name: "rom.bin".into(),
        size_bytes: Some(ROM_BYTES.len() as u64),
        crc32: None,
        md5: None,
        sha1: Some(rom_sha1()),
    }
}

fn matching_disk() -> MameObservedMember {
    disk_member(&chd_header(false))
}

fn chd_header(parent_required: bool) -> Vec<u8> {
    let mut bytes = vec![0; 124];
    bytes[..8].copy_from_slice(b"MComprHD");
    bytes[8..12].copy_from_slice(&124_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(&5_u32.to_be_bytes());
    bytes[32..40].copy_from_slice(&8192_u64.to_be_bytes());
    bytes[40..48].copy_from_slice(&124_u64.to_be_bytes());
    bytes[56..60].copy_from_slice(&4096_u32.to_be_bytes());
    bytes[60..64].copy_from_slice(&512_u32.to_be_bytes());
    bytes[84..104].fill(0xaa);
    if parent_required {
        bytes[104..124].fill(0xbb);
    }
    bytes
}

fn disk_member(bytes: &[u8]) -> MameObservedMember {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("disk.chd");
    std::fs::write(&path, bytes).unwrap();
    // Same historical matching-header bytes; current inventory deliberately
    // has no header-as-verification field (removed by #159).
    let header = read_chd_v5_header(&mut std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(header.overall_sha1, [0xaa; 20]);
    MameObservedMember {
        name: "disk.chd".into(),
        size_bytes: Some(bytes.len() as u64),
        crc32: None,
        md5: None,
        sha1: None,
    }
}

fn observed(name: &str, members: Vec<MameObservedMember>) -> MameObservedSet {
    MameObservedSet {
        name: name.into(),
        location: PathBuf::from(format!("/synthetic/{name}")),
        directory: true,
        members,
    }
}

fn inventory(sets: Vec<MameObservedSet>) -> MameCollectionInventory {
    MameCollectionInventory {
        collection_root: "/synthetic".into(),
        sets,
        inspected_completely: true,
        warnings: vec![],
    }
}

fn plan(dat: &ParsedDat, inventory: &MameCollectionInventory) -> MamePlayingLibraryPlan {
    let analysis = analyse_collection(dat, inventory, None, None);
    build_mame_playing_library_plan(&MamePlayingLibraryRequest {
        catalogue: dat,
        inventory,
        analysis: &analysis,
        preferences: Default::default(),
    })
    .unwrap()
}

fn assert_withheld(plan: &MamePlayingLibraryPlan, name: &str) {
    assert!(
        !plan
            .selected_sets
            .iter()
            .any(|set| set.name == name && set.complete),
        "{name} incorrectly selected complete: {:?}",
        plan.selected_sets
    );
    assert!(
        !plan.selected_sets.iter().any(|set| set.name == name),
        "{name} selected, with a preservation-complete claim: {:?}",
        plan.selected_sets
    );
    assert!(plan.excluded_sets.iter().any(|set| set.name == name));
}

fn assert_selected(plan: &MamePlayingLibraryPlan, name: &str) {
    let selected = plan
        .selected_sets
        .iter()
        .find(|set| set.name == name)
        .unwrap_or_else(|| panic!("{name} unexpectedly excluded: {:?}", plan.excluded_sets));
    assert!(selected.complete);
}

#[test]
fn parsed_required_device_chd_cannot_select_a_good_rom_only_host() {
    let dat = parsed(&format!(
        "<machine name=\"diskdevice\" isdevice=\"yes\">{}</machine>
         <machine name=\"dependent\">{}<device_ref name=\"diskdevice\"/></machine>",
        disk_xml(),
        rom_xml()
    ));
    for members in [vec![matching_disk()], vec![]] {
        let inventory = inventory(vec![
            observed("diskdevice", members),
            observed("dependent", vec![rom_member()]),
        ]);
        let analysis = analyse_collection(&dat, &inventory, None, None);
        println!(
            "current analyser health: {:?}",
            analysis
                .set_results
                .iter()
                .find(|r| r.name == "dependent")
                .unwrap()
                .health
        );
        assert_withheld(&plan(&dat, &inventory), "dependent");
    }
}

#[test]
fn parsed_required_parent_chd_cannot_select_a_good_rom_only_clone() {
    let dat = parsed(&format!(
        "<machine name=\"parent\">{}</machine>
         <machine name=\"clone\" cloneof=\"parent\" romof=\"parent\">{}</machine>",
        disk_xml(),
        rom_xml()
    ));
    let inventory = inventory(vec![
        observed("parent", vec![matching_disk()]),
        observed("clone", vec![rom_member()]),
    ]);
    let analysis = analyse_collection(&dat, &inventory, None, None);
    println!(
        "current analyser health: {:?}",
        analysis
            .set_results
            .iter()
            .find(|r| r.name == "clone")
            .unwrap()
            .health
    );
    assert_withheld(&plan(&dat, &inventory), "clone");
}

#[test]
fn required_parent_bios_device_rom_health_blocks_selection() {
    for kind in ["parent", "bios", "device"] {
        let (flags, host_flags, reference) = match kind {
            "parent" => ("", "romof=\"support\" cloneof=\"support\"", ""),
            "bios" => ("isbios=\"yes\"", "romof=\"support\"", ""),
            _ => ("isdevice=\"yes\"", "", "<device_ref name=\"support\"/>"),
        };
        let support_rom = rom_xml().replace("rom.bin", "support.bin");
        let dat = parsed(&format!(
            "<machine name=\"support\" {flags}>{support_rom}</machine><machine name=\"host\" {host_flags}>{}{reference}</machine>",
            rom_xml()
        ));
        for corrupt in [false, true] {
            let members = if corrupt {
                let mut r = rom_member();
                r.name = "support.bin".into();
                r.sha1 = Some("b".repeat(40));
                vec![r]
            } else {
                vec![]
            };
            let inv = inventory(vec![
                observed("host", vec![rom_member()]),
                observed("support", members),
            ]);
            assert_withheld(&plan(&dat, &inv), "host");
        }
    }
}

#[test]
fn required_parent_device_chain_blocks_selection() {
    let dat = parsed(&format!(
        "<machine name=\"leaf\" isdevice=\"yes\">{}</machine><machine name=\"parent\"><device_ref name=\"leaf\"/></machine><machine name=\"host\" romof=\"parent\">{}</machine>",
        disk_xml(),
        rom_xml()
    ));
    let inv = inventory(vec![
        observed("host", vec![rom_member()]),
        observed("leaf", vec![matching_disk()]),
    ]);
    assert_withheld(&plan(&dat, &inv), "host");
}

#[test]
fn parent_uncertainty_and_missing_storage_never_select_complete() {
    for material in [
        disk_xml(),
        "<rom name=\"unknown.bin\" status=\"nodump\"/>".into(),
        "<rom name=\"unknown.bin\" size=\"4\"/>".into(),
    ] {
        let dat = parsed(&format!(
            "<machine name=\"parent\">{material}</machine><machine name=\"host\" romof=\"parent\">{}</machine>",
            rom_xml()
        ));
        for present in [false, true] {
            let mut sets = vec![observed("host", vec![rom_member()])];
            if present {
                sets.push(observed("parent", vec![rom_member()]));
            }
            assert_withheld(&plan(&dat, &inventory(sets)), "host");
        }
    }
}

#[test]
fn verified_parent_chd_is_eligible_without_claiming_runnability() {
    let dat = parsed(&format!(
        "<machine name=\"parent\">{}</machine><machine name=\"host\" romof=\"parent\">{}</machine>",
        disk_xml(),
        rom_xml()
    ));
    let inv = inventory(vec![
        observed("host", vec![rom_member()]),
        observed("parent", vec![matching_disk()]),
    ]);
    let analysis = analyse_collection_with_chds(
        &dat,
        &inv,
        None,
        None,
        &[MameVerifiedChd {
            set_name: "parent".into(),
            name: "disk".into(),
            verified_overall_sha1: DISK_SHA1.into(),
        }],
    );
    let plan = build_mame_playing_library_plan(&MamePlayingLibraryRequest {
        catalogue: &dat,
        inventory: &inv,
        analysis: &analysis,
        preferences: Default::default(),
    })
    .unwrap();
    assert_selected(&plan, "host");
    assert!(
        analysis
            .set_results
            .iter()
            .all(|r| r.evidence.runnability == MameRunnability::NotAssessed)
    );
}

#[test]
fn parsed_missing_required_support_storage_blocks_selection() {
    for relationship in ["<device_ref name=\"support\"/>", ""] {
        let attributes = if relationship.is_empty() {
            "romof=\"support\""
        } else {
            ""
        };
        let dat = parsed(&format!(
            "<machine name=\"support\" isdevice=\"yes\">{}</machine>
             <machine name=\"host\" {attributes}>{}{relationship}</machine>",
            rom_xml().replace("rom.bin", "support.bin"),
            rom_xml()
        ));
        assert_withheld(
            &plan(&dat, &inventory(vec![observed("host", vec![rom_member()])])),
            "host",
        );
    }
}

#[test]
fn parsed_missing_malformed_ambiguous_and_cyclic_references_never_become_complete() {
    let cases = [
        "<machine name=\"host\"><device_ref name=\"absent\"/></machine>".to_string(),
        "<machine name=\"host\"><device_ref/></machine>".to_string(),
        "<machine name=\"host\"><device_ref name=\" \"/></machine>".to_string(),
        "<machine name=\"host\" romof=\"absent\"/>".to_string(),
        "<machine name=\"host\" cloneof=\"absent\"/>".to_string(),
        "<machine name=\"host\" cloneof=\" \"/>".to_string(),
        "<machine name=\"host\" romof=\"host\"/>".to_string(),
        "<machine name=\"host\" cloneof=\"other\"/><machine name=\"other\" cloneof=\"host\"/>".to_string(),
        "<machine name=\"host\"><device_ref name=\"dev\"/></machine><machine name=\"dev\" isdevice=\"yes\"><device_ref name=\"other\"/></machine><machine name=\"other\" isdevice=\"yes\"><device_ref name=\"dev\"/></machine>".to_string(),
        "<machine name=\"host\" romof=\"dev\"/><machine name=\"dev\" isdevice=\"yes\"><device_ref name=\"host\"/></machine>".to_string(),
        "<machine name=\"host\"><device_ref name=\"dev\"/></machine><machine name=\"dev\" isdevice=\"yes\"/><machine name=\"dev\" isdevice=\"yes\"/>".to_string(),
        "<machine name=\"host\"><device_ref name=\"impostor\"/></machine><machine name=\"impostor\"/>".to_string(),
    ];
    for xml in cases {
        let dat = parsed(&xml);
        let inventory = inventory(vec![observed("host", vec![])]);
        assert_withheld(&plan(&dat, &inventory), "host");
    }
}

#[test]
fn parsed_optional_slot_device_is_not_a_required_support_set() {
    let dat = parsed(&format!(
        "<machine name=\"optionaldev\" isdevice=\"yes\">{}</machine>
         <machine name=\"host\">{}<slot name=\"expansion\"><slotoption name=\"cd\" devname=\"optionaldev\" default=\"no\"/></slot></machine>", disk_xml(), rom_xml()
    ));
    assert!(dat.games[1].device_refs.is_empty());
    let plan = plan(&dat, &inventory(vec![observed("host", vec![rom_member()])]));
    assert_selected(&plan, "host");
    assert!(plan.required_support_sets.is_empty());
}

#[test]
fn parsed_self_contained_clone_does_not_require_unrelated_parent_storage() {
    let dat = parsed(&format!(
        "<machine name=\"parent\">{}</machine><machine name=\"host\" cloneof=\"parent\">{}</machine>",
        disk_xml(),
        rom_xml()
    ));
    let plan = plan(&dat, &inventory(vec![observed("host", vec![rom_member()])]));
    assert_selected(&plan, "host");
    assert!(plan.required_support_sets.is_empty());
}

#[test]
fn parsed_rom_only_dependency_diamond_is_selected_and_support_closure_is_reported() {
    let dat = parsed(&format!(
        "<machine name=\"leaf\" isdevice=\"yes\">{}</machine>
         <machine name=\"left\" isdevice=\"yes\"><device_ref name=\"leaf\"/></machine>
         <machine name=\"right\" isdevice=\"yes\"><device_ref name=\"leaf\"/></machine>
         <machine name=\"host\">{}<device_ref name=\"left\"/><device_ref name=\"right\"/></machine>", rom_xml(), rom_xml()
    ));
    let plan = plan(
        &dat,
        &inventory(vec![
            observed("host", vec![rom_member()]),
            observed("leaf", vec![rom_member()]),
        ]),
    );
    assert_selected(&plan, "host");
    assert_eq!(plan.selected_sets[0].required_dependencies, ["leaf"]);
    assert_eq!(plan.required_support_sets.len(), 1);
    assert_eq!(plan.required_support_sets[0].name, "leaf");
    assert!(plan.required_support_sets[0].available);
}

#[test]
fn parsed_required_rom_only_support_must_have_good_health() {
    let dat = parsed(&format!(
        "<machine name=\"dev\" isdevice=\"yes\">{}</machine><machine name=\"host\">{}<device_ref name=\"dev\"/></machine>",
        rom_xml().replace("rom.bin", "support.bin"),
        rom_xml()
    ));
    let mut corrupt = rom_member();
    corrupt.name = "support.bin".into();
    corrupt.sha1 = Some("b".repeat(40));
    let inventory = inventory(vec![
        observed("host", vec![rom_member()]),
        observed("dev", vec![corrupt]),
    ]);
    assert_withheld(&plan(&dat, &inventory), "host");
}

#[test]
fn parsed_verified_rom_only_machine_keeps_existing_selection() {
    let dat = parsed(&format!("<machine name=\"host\">{}</machine>", rom_xml()));
    let inventory = inventory(vec![observed("host", vec![rom_member()])]);
    let analysis = analyse_collection(&dat, &inventory, None, None);
    assert_eq!(analysis.set_results[0].matched_members, 1);
    assert_eq!(analysis.set_results[0].health, MameSetHealth::Good);
    let plan = plan(&dat, &inventory);
    assert_selected(&plan, "host");
    assert!(plan.excluded_sets.is_empty());
}

#[test]
fn parsed_support_chd_is_withheld_even_if_supplied_health_claims_good() {
    let dat = parsed(&format!(
        "<machine name=\"dev\" isdevice=\"yes\">{}</machine><machine name=\"host\">{}<device_ref name=\"dev\"/></machine>",
        disk_xml(),
        rom_xml()
    ));
    let inventory = inventory(vec![
        observed("host", vec![rom_member()]),
        observed("dev", vec![matching_disk()]),
    ]);
    let mut analysis = analyse_collection(&dat, &inventory, None, None);
    for set in &mut analysis.set_results {
        set.health = MameSetHealth::Good;
    }
    let plan = build_mame_playing_library_plan(&MamePlayingLibraryRequest {
        catalogue: &dat,
        inventory: &inventory,
        analysis: &analysis,
        preferences: Default::default(),
    })
    .unwrap();
    assert_withheld(&plan, "host");
}

#[test]
fn parsed_optional_rom_only_support_does_not_require_an_archive() {
    let dat = parsed(&format!(
        "<machine name=\"dev\" isdevice=\"yes\"><rom name=\"optional.bin\" size=\"4\" sha1=\"{}\" optional=\"yes\"/></machine>
         <machine name=\"host\">{}<device_ref name=\"dev\"/></machine>", rom_sha1(), rom_xml()
    ));
    assert_selected(
        &plan(&dat, &inventory(vec![observed("host", vec![rom_member()])])),
        "host",
    );
}

#[test]
fn parsed_dependency_depth_exhaustion_is_unresolved() {
    let mut xml = format!(
        "<machine name=\"host\">{}<device_ref name=\"dev0\"/></machine>",
        rom_xml()
    );
    for index in 0..70 {
        let reference = if index == 69 {
            String::new()
        } else {
            format!("<device_ref name=\"dev{}\"/>", index + 1)
        };
        xml.push_str(&format!(
            "<machine name=\"dev{index}\" isdevice=\"yes\">{reference}</machine>"
        ));
    }
    let dat = parsed(&xml);
    let plan = plan(&dat, &inventory(vec![observed("host", vec![rom_member()])]));
    assert_withheld(&plan, "host");
}

#[test]
fn parsed_rom_source_takes_precedence_over_hierarchy_for_storage() {
    let dat = parsed(&format!(
        "<machine name=\"parent\">{}</machine>
         <machine name=\"source\">{}</machine>
         <machine name=\"host\" cloneof=\"parent\" romof=\"source\"><rom name=\"rom.bin\" size=\"4\" sha1=\"{}\" merge=\"rom.bin\"/></machine>", disk_xml(), rom_xml(), rom_sha1()
    ));
    let plan = plan(
        &dat,
        &inventory(vec![
            observed("host", vec![rom_member()]),
            observed("source", vec![rom_member()]),
        ]),
    );
    assert_selected(&plan, "host");
    let host = plan
        .selected_sets
        .iter()
        .find(|set| set.name == "host")
        .unwrap();
    assert_eq!(host.required_dependencies, ["source"]);
    assert!(
        plan.required_support_sets
            .iter()
            .all(|set| set.name != "parent")
    );
}

#[test]
fn effective_host_bios_choice_preserves_verified_support() {
    let dat = parsed(&format!(
        "<machine name=\"bios\" isbios=\"yes\"><biosset name=\"a\" default=\"yes\"/><biosset name=\"b\"/><rom name=\"a.bin\" bios=\"a\" size=\"4\" sha1=\"{}\"/><rom name=\"b.bin\" bios=\"b\" size=\"4\" sha1=\"{}\"/></machine><machine name=\"host\" romof=\"bios\"><biosset name=\"a\"/><biosset name=\"b\" default=\"yes\"/>{}</machine>",
        rom_sha1(),
        rom_sha1(),
        rom_xml()
    ));
    let mut member = rom_member();
    member.name = "b.bin".into();
    let inv = inventory(vec![
        observed("host", vec![rom_member()]),
        observed("bios", vec![member]),
    ]);
    let analysis = analyse_collection(&dat, &inv, None, None);
    assert_eq!(
        analysis
            .set_results
            .iter()
            .find(|r| r.name == "bios")
            .unwrap()
            .health,
        MameSetHealth::Bad
    );
    assert_eq!(
        analysis
            .set_results
            .iter()
            .find(|r| r.name == "host")
            .unwrap()
            .health,
        MameSetHealth::Good
    );
    assert_selected(&plan(&dat, &inv), "host");
}

#[test]
fn verified_nonmerged_host_copy_does_not_require_a_device_archive() {
    let dat = parsed(&format!(
        "<machine name=\"dev\" isdevice=\"yes\">{}</machine><machine name=\"host\">{}<device_ref name=\"dev\"/></machine>",
        rom_xml(),
        rom_xml()
    ));
    let inv = inventory(vec![observed("host", vec![rom_member()])]);
    assert_selected(&plan(&dat, &inv), "host");
}

#[test]
fn lineage_only_parent_keeps_159_dependency_separation() {
    for reference in [
        "<device_ref name=\"absent\"/>",
        "<device_ref name=\"parent\"/>",
    ] {
        let dat = parsed(&format!(
            "<machine name=\"parent\" romof=\"absent\">{reference}</machine><machine name=\"host\" cloneof=\"parent\">{}</machine>",
            rom_xml()
        ));
        let inv = inventory(vec![observed("host", vec![rom_member()])]);
        assert_selected(&plan(&dat, &inv), "host");
    }
}

#[test]
fn cloneof_bios_ancestry_still_requires_verified_material() {
    let dat = parsed(&format!(
        "<machine name=\"bios\" isbios=\"yes\">{}</machine><machine name=\"middle\" cloneof=\"bios\"/><machine name=\"host\" cloneof=\"middle\">{}</machine>",
        disk_xml(),
        rom_xml()
    ));
    assert_withheld(
        &plan(&dat, &inventory(vec![observed("host", vec![rom_member()])])),
        "host",
    );
}

#[test]
fn inactive_bios_material_does_not_require_empty_selected_variant_health() {
    for optional in ["", "<rom name=\"b.bin\" bios=\"b\" optional=\"yes\"/>"] {
        let dat = parsed(&format!(
            "<machine name=\"bios\" isbios=\"yes\"><biosset name=\"a\"/><biosset name=\"b\" default=\"yes\"/><rom name=\"a.bin\" bios=\"a\" size=\"4\" sha1=\"{}\"/>{optional}</machine><machine name=\"host\" romof=\"bios\">{}</machine>",
            rom_sha1(),
            rom_xml()
        ));
        let inv = inventory(vec![observed("host", vec![rom_member()])]);
        let analysis = analyse_collection(&dat, &inv, None, None);
        assert_eq!(
            analysis
                .set_results
                .iter()
                .find(|r| r.name == "host")
                .unwrap()
                .health,
            MameSetHealth::Good
        );
        assert_selected(&plan(&dat, &inv), "host");
    }
}

#[test]
fn verified_borrowed_alias_covers_only_its_exact_parent_material() {
    for extra in [
        "".to_string(),
        disk_xml(),
        format!(
            "<rom name=\"extra.bin\" size=\"4\" sha1=\"{}\"/>",
            rom_sha1()
        ),
    ] {
        let dat = parsed(&format!(
            "<machine name=\"parent\">{}{extra}</machine><machine name=\"host\" cloneof=\"parent\" romof=\"parent\"><rom name=\"alias.bin\" merge=\"rom.bin\" size=\"4\" sha1=\"{}\"/></machine>",
            rom_xml(),
            rom_sha1()
        ));
        let mut alias = rom_member();
        alias.name = "alias.bin".into();
        let inv = inventory(vec![observed("host", vec![alias])]);
        let plan = plan(&dat, &inv);
        if extra.is_empty() {
            assert_selected(&plan, "host");
        } else {
            assert_withheld(&plan, "host");
        }
    }
}
