//! Parent, BIOS and device inheritance for merged reconstruction. Synthetic
//! catalogues and tiny ZIPs in temporary directories only.
use super::*;
use crate::dat::model::{
    DatDeviceRefEntry, DatDiskEntry, DatEcosystem, DatFormat, DatGameEntry, DatPackingPolicy,
    DatSource,
};
use sha1::Digest;
use std::io::{Read, Write};

mod mame_correctness {
    use super::super::dependencies::{FamilyCatalogue, MemberPlacement};
    use super::*;

    #[test]
    fn mame_correctness_device_borrow_uses_actual_external_rom_owner() {
        let mut cpu = machine(
            "cpu",
            None,
            Some("firmware"),
            vec![rom("alias", BIOS, Some("fw"))],
        );
        cpu.is_device = Some("yes".into());
        let mut parent = machine("parent", None, None, vec![rom("game", PARENT, None)]);
        parent.device_refs.push(DatDeviceRefEntry {
            name: Some("cpu".into()),
        });
        let dat = catalogue(vec![
            parent,
            machine("child", Some("parent"), None, vec![]),
            cpu,
            machine("firmware", None, None, vec![rom("fw", BIOS, None)]),
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        let storage = report
            .stored_separately()
            .map(|s| (s.name.as_str(), s.kind))
            .collect::<Vec<_>>();
        assert_eq!(
            storage,
            vec![("firmware", ExternalSetKind::RomSource)],
            "the final ROM owner requires storage; the alias-only device does not: {report:#?}"
        );
    }

    #[test]
    fn mame_correctness_device_borrow_conflicts_block_reconstruction() {
        let mut cpu = machine(
            "cpu",
            None,
            Some("firmware"),
            vec![rom("alias", BIOS, Some("fw"))],
        );
        cpu.is_device = Some("yes".into());
        let mut parent = machine("parent", None, None, vec![rom("game", PARENT, None)]);
        parent.device_refs.push(DatDeviceRefEntry {
            name: Some("cpu".into()),
        });
        let dat = catalogue(vec![
            parent,
            machine(
                "child",
                Some("parent"),
                None,
                vec![rom("child", CLONE, None)],
            ),
            cpu,
            machine(
                "firmware",
                None,
                None,
                vec![rom("fw", b"conflicting firmware", None)],
            ),
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert!(!report.unresolved_relationships.is_empty(), "{report:#?}");
        let plan =
            build_merged_reconstruction_plan(Path::new("/output"), &dat, &[], "parent", "digest")
                .unwrap();
        assert!(!plan.unresolved_ownership.is_empty());
    }

    fn device(name: &str, rom_of: Option<&str>, roms: Vec<DatRomEntry>) -> DatGameEntry {
        let mut game = machine(name, None, rom_of, roms);
        game.is_device = Some("yes".into());
        game
    }

    fn referencing(devices: &[&str]) -> DatGameEntry {
        let mut parent = machine("parent", None, None, vec![rom("game", PARENT, None)]);
        parent.device_refs = devices
            .iter()
            .map(|name| DatDeviceRefEntry {
                name: Some((*name).into()),
            })
            .collect();
        parent
    }

    fn with_clone(games: Vec<DatGameEntry>) -> ParsedDat {
        let mut all = games;
        all.push(machine("child", Some("parent"), None, vec![]));
        catalogue(all)
    }

    #[test]
    fn mame_correctness_device_own_rom_is_required_with_provenance() {
        let dat = with_clone(vec![
            referencing(&["cpu"]),
            device("cpu", None, vec![rom("cpu.bin", BIOS, None)]),
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert_eq!(
            report
                .stored_separately()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["cpu"]
        );
        assert_eq!(
            report.device_members,
            vec![DeviceMember {
                device: "cpu".into(),
                member_name: "cpu.bin".into(),
                supply: DeviceMemberSupply::DeviceOwn,
                needed_by: vec!["parent".into()],
            }]
        );
        assert!(report.unresolved_relationships.is_empty(), "{report:#?}");
    }

    #[test]
    fn mame_correctness_unrelated_device_rom_is_not_inherited() {
        let dat = with_clone(vec![
            referencing(&["cpu"]),
            device("cpu", None, vec![rom("cpu.bin", BIOS, None)]),
            device("other", None, vec![rom("other.bin", CLONE, None)]),
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert!(report.external_sets.iter().all(|s| s.name != "other"));
        assert!(report.device_members.iter().all(|m| m.device != "other"));
    }

    #[test]
    fn mame_correctness_missing_or_duplicate_device_fails_closed() {
        let missing = with_clone(vec![referencing(&["ghost"])]);
        let report = reconstruction_dependencies(&missing, "parent").unwrap();
        assert!(!report.unresolved_relationships.is_empty(), "{report:#?}");
        let plan = build_merged_reconstruction_plan(
            Path::new("/output"),
            &missing,
            &[],
            "parent",
            "digest",
        )
        .unwrap();
        assert!(!plan.unresolved_ownership.is_empty());

        let duplicate = with_clone(vec![
            referencing(&["cpu"]),
            device("cpu", None, vec![rom("a", BIOS, None)]),
            device("cpu", None, vec![rom("b", CLONE, None)]),
        ]);
        let report = reconstruction_dependencies(&duplicate, "parent").unwrap();
        assert!(!report.unresolved_relationships.is_empty(), "{report:#?}");
        assert!(report.device_members.is_empty(), "{report:#?}");
    }

    #[test]
    fn mame_correctness_device_dependency_cycle_is_refused() {
        let mut a = device("a", None, vec![rom("a.bin", BIOS, None)]);
        a.device_refs = vec![DatDeviceRefEntry {
            name: Some("b".into()),
        }];
        let mut b = device("b", None, vec![rom("b.bin", CLONE, None)]);
        b.device_refs = vec![DatDeviceRefEntry {
            name: Some("a".into()),
        }];
        let dat = with_clone(vec![referencing(&["a"]), a, b]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert!(
            report
                .unresolved_relationships
                .iter()
                .any(|r| r.contains("cyclic")),
            "{report:#?}"
        );
    }

    #[test]
    fn mame_correctness_explicit_device_merge_keeps_owner_and_bios_stays_distinct() {
        let mut bios = machine("bios", None, None, vec![rom("fw", BIOS, None)]);
        bios.is_bios = Some("yes".into());
        let dat = with_clone(vec![
            referencing(&["cpu"]),
            device("cpu", Some("bios"), vec![rom("alias", BIOS, Some("fw"))]),
            bios,
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert_eq!(
            report.device_members[0].supply,
            DeviceMemberSupply::BorrowedFrom("bios".into())
        );
        let kinds = report
            .external_sets
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect::<Vec<_>>();
        assert!(
            kinds.contains(&("bios", ExternalSetKind::Bios)),
            "{kinds:?}"
        );
        assert!(
            kinds.contains(&("cpu", ExternalSetKind::Device)),
            "{kinds:?}"
        );
        assert_eq!(
            report
                .stored_separately()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["bios"]
        );
    }

    #[test]
    fn mame_correctness_matching_filename_alone_does_not_pick_a_device_owner() {
        // The device declares its own "fw" with different bytes and no merge;
        // the same-named member of an unrelated romof set is not its owner.
        let dat = with_clone(vec![
            referencing(&["cpu"]),
            device("cpu", Some("firmware"), vec![rom("fw", CLONE, None)]),
            machine("firmware", None, None, vec![rom("fw", BIOS, None)]),
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert_eq!(
            report.device_members[0].supply,
            DeviceMemberSupply::DeviceOwn
        );
        assert!(report.unresolved_relationships.is_empty(), "{report:#?}");
    }

    #[test]
    fn mame_correctness_device_merging_into_a_family_set_is_not_device_owned() {
        let dat = with_clone(vec![
            referencing(&["cpu"]),
            device(
                "cpu",
                Some("parent"),
                vec![rom("alias", PARENT, Some("game"))],
            ),
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert!(report.device_members.is_empty(), "{report:#?}");
        assert!(
            report
                .unresolved_relationships
                .iter()
                .any(|r| r.contains("family set")),
            "{report:#?}"
        );
    }

    #[test]
    fn mame_correctness_reconstruction_reports_transitive_bios_support() {
        let mut cpu = machine("cpu", None, Some("bios"), vec![rom("cpu", BIOS, None)]);
        cpu.is_device = Some("yes".into());
        let mut bios = machine("bios", None, None, vec![rom("firmware", BIOS, None)]);
        bios.is_bios = Some("yes".into());
        let mut parent = machine("parent", None, None, vec![rom("game", PARENT, None)]);
        parent.device_refs.push(DatDeviceRefEntry {
            name: Some("cpu".into()),
        });
        let dat = catalogue(vec![
            parent,
            machine("child", Some("parent"), None, vec![]),
            cpu,
            bios,
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert!(
            report
                .external_sets
                .iter()
                .any(|set| set.name == "bios" && set.kind == ExternalSetKind::Bios)
        );
    }

    #[test]
    fn mame_correctness_unused_ordinary_rom_source_is_not_required_storage() {
        let dat = catalogue(vec![
            machine(
                "parent",
                None,
                Some("ordinary"),
                vec![rom("game", PARENT, None)],
            ),
            machine("child", Some("parent"), None, vec![]),
            machine("ordinary", None, None, vec![rom("unused", BIOS, None)]),
        ]);
        let report = reconstruction_dependencies(&dat, "parent").unwrap();
        assert!(!report.stored_separately().any(|set| set.name == "ordinary"));
    }

    #[test]
    fn mame_correctness_invalid_support_relationships_are_reported_unresolved() {
        let mut failures = Vec::new();
        for kind in ["unnamed-device", "cyclic-device"] {
            let mut parent = machine("parent", None, None, vec![rom("game", PARENT, None)]);
            let mut cpu = machine("cpu", None, None, vec![]);
            cpu.is_device = Some("yes".into());
            if kind == "unnamed-device" {
                parent.device_refs.push(DatDeviceRefEntry { name: None });
            } else {
                parent.device_refs.push(DatDeviceRefEntry {
                    name: Some("cpu".into()),
                });
                cpu.device_refs.push(DatDeviceRefEntry {
                    name: Some("cpu".into()),
                });
            }
            let dat = catalogue(vec![
                parent,
                machine("child", Some("parent"), None, vec![]),
                cpu,
            ]);
            let report =
                serde_json::to_value(reconstruction_dependencies(&dat, "parent").unwrap()).unwrap();
            if !report
                .get("unresolved_relationships")
                .and_then(|v| v.as_array())
                .is_some_and(|v| !v.is_empty())
            {
                failures.push(kind);
            }
        }
        assert!(
            failures.is_empty(),
            "silently omitted malformed relationships: {failures:?}"
        );
    }

    #[test]
    fn mame_correctness_reconstruction_merge_checks_the_entire_chain() {
        let mut first = rom("first", PARENT, Some("middle"));
        first.crc32 = Some("11111111".into());
        let mut middle = rom("middle", PARENT, Some("last"));
        middle.crc32 = None;
        let mut last = rom("last", PARENT, None);
        last.crc32 = Some("22222222".into());
        let dat = catalogue(vec![
            machine("child", Some("root"), Some("middle"), vec![first]),
            machine("middle", Some("root"), Some("root"), vec![middle]),
            machine("root", None, None, vec![last]),
        ]);
        let lookup = FamilyCatalogue::new(&dat, ["root", "middle", "child"]);
        assert!(matches!(
            lookup.placement(&dat.games[0], &dat.games[0].roms[0]),
            MemberPlacement::Unresolved(_)
        ));
    }

    #[test]
    fn mame_correctness_external_owner_needs_comparable_content() {
        let mut borrowed = rom("alias", PARENT, Some("owner"));
        borrowed.crc32 = None;
        let mut owner = rom("owner", PARENT, None);
        owner.sha1 = None;
        let dat = catalogue(vec![
            machine("parent", None, Some("bios"), vec![borrowed]),
            machine(
                "child",
                Some("parent"),
                None,
                vec![rom("game", CLONE, None)],
            ),
            machine("bios", None, None, vec![owner]),
        ]);
        let lookup = FamilyCatalogue::new(&dat, ["parent", "child"]);
        assert!(matches!(
            lookup.placement(&dat.games[0], &dat.games[0].roms[0]),
            MemberPlacement::Unresolved(_)
        ));
    }

    #[test]
    fn mame_correctness_external_merge_chain_faults_are_not_hidden() {
        let mut failures = Vec::new();
        for kind in ["missing", "cycle", "duplicate", "conflict"] {
            let parent = machine(
                "parent",
                None,
                Some("bios"),
                vec![rom("alias", PARENT, Some("bios-alias"))],
            );
            let child = machine(
                "child",
                Some("parent"),
                None,
                vec![rom("game", CLONE, None)],
            );
            let bios = machine(
                "bios",
                None,
                Some("root"),
                vec![rom("bios-alias", PARENT, Some("owner"))],
            );
            let mut games = vec![parent, child, bios];
            if kind != "missing" {
                games.push(machine(
                    "root",
                    None,
                    None,
                    vec![rom("owner", PARENT, None)],
                ));
            }
            match kind {
                "cycle" => {
                    games[3].rom_of = Some("bios".into());
                    games[3].roms[0].merge = Some("bios-alias".into());
                }
                "duplicate" => games.push(games[3].clone()),
                "conflict" => games[3].roms[0].sha1 = Some("f".repeat(40)),
                _ => {}
            }
            let dat = catalogue(games);
            let lookup = FamilyCatalogue::new(&dat, ["parent", "child"]);
            if !matches!(
                lookup.placement(&dat.games[0], &dat.games[0].roms[0]),
                MemberPlacement::Unresolved(_)
            ) {
                failures.push(kind);
            }
        }
        assert!(
            failures.is_empty(),
            "external ownership incorrectly accepted: {failures:?}"
        );
    }

    #[test]
    fn mame_correctness_family_identity_refuses_ambiguous_or_nonflat_lineage() {
        let mut failures = Vec::new();
        for kind in [
            "duplicate-parent",
            "duplicate-child",
            "missing-parent",
            "self",
            "cycle",
            "nested-parent",
            "nested-child",
        ] {
            let mut games = vec![
                machine("parent", None, None, vec![rom("main", PARENT, None)]),
                machine(
                    "child",
                    Some("parent"),
                    Some("parent"),
                    vec![rom("game", CLONE, None)],
                ),
            ];
            match kind {
                "duplicate-parent" => games.push(games[0].clone()),
                "duplicate-child" => games.push(games[1].clone()),
                "missing-parent" => {
                    games.remove(0);
                }
                "self" => games[0].clone_of = Some("parent".into()),
                "cycle" => games[0].clone_of = Some("child".into()),
                "nested-parent" => {
                    games[0].clone_of = Some("root".into());
                    games.push(machine("root", None, None, vec![]));
                }
                "nested-child" => {
                    games.push(machine("grandchild", Some("child"), Some("child"), vec![]))
                }
                _ => unreachable!(),
            }
            let dat = catalogue(games);
            if reconstruction_family_names(&dat, "child").is_ok() {
                failures.push(kind);
            }
        }
        assert!(
            failures.is_empty(),
            "ambiguous or unsupported family accepted: {failures:?}"
        );
    }
}

fn sha1_hex(bytes: &[u8]) -> String {
    sha1::Sha1::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn crc32_hex(bytes: &[u8]) -> String {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    format!("{:08x}", !crc)
}

fn rom(name: &str, bytes: &[u8], merge: Option<&str>) -> DatRomEntry {
    DatRomEntry {
        name: name.into(),
        size_bytes: Some(bytes.len() as u64),
        sha1: Some(sha1_hex(bytes)),
        crc32: Some(crc32_hex(bytes)),
        merge: merge.map(str::to_owned),
        ..Default::default()
    }
}

fn machine(
    name: &str,
    clone_of: Option<&str>,
    rom_of: Option<&str>,
    roms: Vec<DatRomEntry>,
) -> DatGameEntry {
    DatGameEntry {
        name: name.into(),
        clone_of: clone_of.map(str::to_owned),
        rom_of: rom_of.map(str::to_owned),
        roms,
        ..Default::default()
    }
}

fn catalogue(games: Vec<DatGameEntry>) -> ParsedDat {
    ParsedDat {
        source: DatSource {
            format: DatFormat::Logiqx,
            ecosystem: DatEcosystem::MAMEArcade,
            file_path: "fixture".into(),
            name: None,
            description: None,
            version: Some("test".into()),
            author: None,
            homepage: None,
            clrmamepro_header: None,
            entry_count: games.len(),
            rom_count: games.iter().map(|g| g.roms.len()).sum(),
            parse_warnings: vec![],
            packing_policy: DatPackingPolicy::Standard,
        },
        games,
    }
}

const BIOS: &[u8] = b"bios rom bytes";
const PARENT: &[u8] = b"parent program";
const CLONE: &[u8] = b"clone program";

/// neogeo-style: a BIOS set, a parent borrowing a BIOS ROM through
/// `romof`+`merge`, and a clone borrowing the same ROM through the parent.
fn bios_family() -> ParsedDat {
    let mut bios = machine("bios", None, None, vec![rom("sys.rom", BIOS, None)]);
    bios.is_bios = Some("yes".into());
    catalogue(vec![
        bios,
        machine(
            "parent",
            None,
            Some("bios"),
            vec![
                rom("sys.rom", BIOS, Some("sys.rom")),
                rom("p.rom", PARENT, None),
            ],
        ),
        machine(
            "clone",
            Some("parent"),
            Some("parent"),
            vec![
                rom("sys.rom", BIOS, Some("sys.rom")),
                rom("p.rom", PARENT, Some("p.rom")),
                rom("c.rom", CLONE, None),
            ],
        ),
    ])
}

fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, bytes) in entries {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

fn read_back(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        out.insert(entry.name().unwrap().into_owned(), bytes);
    }
    out
}

#[test]
fn bios_members_are_not_required_in_the_merged_output() {
    let dat = bios_family();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("out")).unwrap();
    // Non-merged donors: both carry the BIOS ROM as well as their own.
    write_zip(
        &root.path().join("parent.zip"),
        &[("sys.rom", BIOS), ("p.rom", PARENT)],
    );
    write_zip(
        &root.path().join("clone.zip"),
        &[("sys.rom", BIOS), ("p.rom", PARENT), ("c.rom", CLONE)],
    );
    let joins = discover_packed_zip_sources(root.path(), &dat, "clone", "digest").unwrap();
    let out = root.path().join("out");
    let plan = build_merged_reconstruction_plan(&out, &dat, &joins, "clone", "digest").unwrap();
    let required = plan
        .required_members
        .iter()
        .map(|r| r.member_name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(required, vec!["c.rom", "p.rom"], "{plan:#?}");
    // The parent ROM is offered by two donors with identical bytes, so it is
    // a duplicate candidate rather than an arbitrary pick.
    assert_eq!(plan.duplicate_candidates, vec!["p.rom".to_string()]);
    assert!(plan.blocked());

    // With one donor per member the plan is ready and the output holds no BIOS ROM.
    std::fs::remove_file(root.path().join("clone.zip")).unwrap();
    write_zip(
        &root.path().join("clone.zip"),
        &[("sys.rom", BIOS), ("c.rom", CLONE)],
    );
    let joins = discover_packed_zip_sources(root.path(), &dat, "clone", "digest").unwrap();
    let plan = build_merged_reconstruction_plan(&out, &dat, &joins, "clone", "digest").unwrap();
    assert!(plan.ready_to_apply, "{plan:#?}");
    let review = review_reconstruction_publication(&plan, None).unwrap();
    let journal = root.path().join("journal");
    let applied = review
        .apply(&root.path().join("stage"), &journal)
        .unwrap()
        .expect("a new output is created");
    assert_eq!(
        applied.transaction.state,
        crate::dat::rename_apply::model::TransactionState::Applied
    );
    let output = read_back(&plan.destination);
    assert_eq!(
        output.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["c.rom", "p.rom"]
    );
    assert_eq!(output["p.rom"], PARENT);
    // Donors are inputs only.
    assert_eq!(
        read_back(&root.path().join("parent.zip"))["sys.rom"],
        BIOS.to_vec()
    );
}

#[test]
fn dependency_report_names_the_bios_and_the_members_it_provides() {
    let report = reconstruction_dependencies(&bios_family(), "parent").unwrap();
    assert_eq!(report.parent, "parent");
    assert_eq!(report.clones, vec!["clone".to_string()]);
    assert_eq!(
        report.external_sets,
        vec![ExternalSupportSet {
            name: "bios".into(),
            kind: ExternalSetKind::Bios,
            catalogue: CatalogueReference::Unique,
            declared_roms: 1,
            needed_by: vec!["clone".into(), "parent".into()],
        }]
    );
    let provided = report
        .excluded_members
        .iter()
        .filter(|m| m.reason == ExcludedMemberReason::ProvidedBy("bios".into()))
        .map(|m| m.declaring_set.as_str())
        .collect::<Vec<_>>();
    assert_eq!(provided, vec!["parent", "clone"]);
    assert!(report.unresolved_members.is_empty());
    assert_eq!(report.stored_separately().count(), 1);
}

#[test]
fn a_merge_target_the_provider_does_not_declare_blocks_the_plan() {
    let mut dat = bios_family();
    dat.games[2].roms[0].merge = Some("absent.rom".into());
    let plan = build_merged_reconstruction_plan(Path::new("/output"), &dat, &[], "clone", "digest")
        .unwrap();
    assert!(plan.unresolved_ownership.contains(&"sys.rom".to_string()));
    assert!(plan.blocked());
    let report = reconstruction_dependencies(&dat, "clone").unwrap();
    assert_eq!(report.unresolved_members.len(), 1);
    assert!(
        report.unresolved_members[0].reason.contains("absent.rom"),
        "{report:#?}"
    );
}

#[test]
fn a_merge_with_a_disagreeing_checksum_is_unresolved_not_borrowed() {
    let mut dat = bios_family();
    dat.games[1].roms[0] = rom("sys.rom", b"different bytes", Some("sys.rom"));
    let report = reconstruction_dependencies(&dat, "parent").unwrap();
    assert!(
        report
            .unresolved_members
            .iter()
            .any(|m| m.declaring_set == "parent" && m.reason.contains("Contradictory")),
        "{report:#?}"
    );
}

#[test]
fn an_absent_romof_provider_is_reported_and_its_members_are_unresolved() {
    let dat = catalogue(vec![
        machine(
            "parent",
            None,
            Some("missingbios"),
            vec![
                rom("sys.rom", BIOS, Some("sys.rom")),
                rom("p.rom", PARENT, None),
            ],
        ),
        machine(
            "clone",
            Some("parent"),
            Some("parent"),
            vec![rom("c.rom", CLONE, None)],
        ),
    ]);
    let report = reconstruction_dependencies(&dat, "clone").unwrap();
    assert_eq!(report.external_sets[0].name, "missingbios");
    assert_eq!(
        report.external_sets[0].catalogue,
        CatalogueReference::Absent
    );
    assert_eq!(report.stored_separately().count(), 1);
    let plan = build_merged_reconstruction_plan(Path::new("/output"), &dat, &[], "clone", "digest")
        .unwrap();
    assert!(plan.unresolved_ownership.contains(&"sys.rom".to_string()));
}

#[test]
fn devices_disks_and_non_physical_members_are_reported_outside_the_output() {
    let mut cpu = machine("cpu", None, None, vec![]);
    cpu.is_device = Some("yes".into());
    cpu.device_refs = vec![DatDeviceRefEntry {
        name: Some("sound".into()),
    }];
    let mut sound = machine("sound", None, None, vec![rom("snd.rom", b"sound", None)]);
    sound.is_device = Some("yes".into());
    let mut parent = machine(
        "parent",
        None,
        None,
        vec![
            rom("p.rom", PARENT, None),
            DatRomEntry {
                name: "gap.rom".into(),
                status: Some("nodump".into()),
                ..Default::default()
            },
            DatRomEntry {
                name: "opt.rom".into(),
                optional: Some("yes".into()),
                ..rom("opt.rom", b"optional", None)
            },
        ],
    );
    parent.device_refs = vec![DatDeviceRefEntry {
        name: Some("cpu".into()),
    }];
    parent.disks = vec![DatDiskEntry {
        name: Some("hdd".into()),
        ..Default::default()
    }];
    parent.sample_of = Some("parent".into());
    let dat = catalogue(vec![
        cpu,
        sound,
        parent,
        machine(
            "clone",
            Some("parent"),
            Some("parent"),
            vec![rom("c.rom", CLONE, None)],
        ),
    ]);
    let report = reconstruction_dependencies(&dat, "parent").unwrap();
    let devices = report
        .external_sets
        .iter()
        .map(|s| (s.name.as_str(), s.kind, s.declared_roms))
        .collect::<Vec<_>>();
    assert_eq!(
        devices,
        vec![
            ("cpu", ExternalSetKind::Device, 0),
            ("sound", ExternalSetKind::Device, 1)
        ]
    );
    // Only a device that declares ROMs must be stored separately.
    assert_eq!(
        report
            .stored_separately()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["sound"]
    );
    assert_eq!(report.disks.len(), 1);
    assert_eq!(report.samples, vec!["parent".to_string()]);
    let reasons = report
        .excluded_members
        .iter()
        .map(|m| (m.member_name.as_str(), m.reason.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        reasons,
        vec![
            ("gap.rom", ExcludedMemberReason::NoDump),
            ("opt.rom", ExcludedMemberReason::Optional)
        ]
    );
    let plan =
        build_merged_reconstruction_plan(Path::new("/output"), &dat, &[], "parent", "digest")
            .unwrap();
    assert_eq!(
        plan.required_members
            .iter()
            .map(|r| r.member_name.as_str())
            .collect::<Vec<_>>(),
        vec!["c.rom", "p.rom"]
    );
}

#[test]
fn a_merge_cycle_is_unresolved_instead_of_looping() {
    let dat = catalogue(vec![
        machine(
            "parent",
            None,
            Some("clone"),
            vec![rom("x.rom", PARENT, Some("x.rom"))],
        ),
        machine(
            "clone",
            Some("parent"),
            Some("parent"),
            vec![rom("x.rom", PARENT, Some("x.rom"))],
        ),
    ]);
    let report = reconstruction_dependencies(&dat, "parent").unwrap();
    assert_eq!(report.unresolved_members.len(), 2, "{report:#?}");
    assert!(report.unresolved_members[0].reason.contains("Cycle"));
}

#[test]
fn a_clone_free_parent_is_standalone_not_incomplete_identity() {
    let dat = catalogue(vec![
        machine("solo", None, None, vec![rom("s.rom", PARENT, None)]),
        machine("parent", None, None, vec![rom("p.rom", PARENT, None)]),
        machine(
            "clone",
            Some("parent"),
            Some("parent"),
            vec![rom("c.rom", CLONE, None)],
        ),
    ]);
    assert_eq!(
        reconstruction_family_shape(&dat, "solo").unwrap(),
        ReconstructionFamilyShape::StandaloneParent {
            parent: "solo".into()
        }
    );
    assert_eq!(
        reconstruction_family_shape(&dat, "clone").unwrap(),
        ReconstructionFamilyShape::ParentWithClones {
            parent: "parent".into(),
            clones: vec!["clone".into()]
        }
    );
    let error = build_merged_reconstruction_plan(Path::new("/output"), &dat, &[], "solo", "digest")
        .unwrap_err();
    assert!(
        error.contains("no merged parent/clone reconstruction is needed for solo"),
        "{error}"
    );
    assert!(!error.contains("incomplete"), "{error}");
    // Dependencies are still reported for a standalone game.
    assert_eq!(
        reconstruction_dependencies(&dat, "solo").unwrap().parent,
        "solo"
    );
    assert!(reconstruction_family_shape(&dat, "absent").is_err());
}

#[test]
fn a_missing_or_duplicated_parent_is_incomplete_identity() {
    let orphan = catalogue(vec![machine(
        "clone",
        Some("gone"),
        Some("gone"),
        vec![rom("c.rom", CLONE, None)],
    )]);
    assert!(matches!(
        reconstruction_family_shape(&orphan, "clone").unwrap(),
        ReconstructionFamilyShape::IncompleteIdentity { reason } if reason.contains("gone is absent")
    ));
    let error =
        build_merged_reconstruction_plan(Path::new("/output"), &orphan, &[], "clone", "digest")
            .unwrap_err();
    assert!(error.contains("parent set gone is absent"), "{error}");

    let twice = catalogue(vec![
        machine("parent", None, None, vec![rom("p.rom", PARENT, None)]),
        machine("parent", None, None, vec![rom("p.rom", PARENT, None)]),
        machine(
            "clone",
            Some("parent"),
            Some("parent"),
            vec![rom("c.rom", CLONE, None)],
        ),
    ]);
    assert!(matches!(
        reconstruction_family_shape(&twice, "clone").unwrap(),
        ReconstructionFamilyShape::IncompleteIdentity { reason } if reason.contains("more than once")
    ));
}
