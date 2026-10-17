//! Disposable CHD headers and real DAT parsing: no production media operations.
use super::*;
use crate::dat::{
    dependency::{
        DependencyState,
        resolve::{CollectionEvidence, resolve_collection},
    },
    disk_audit::audit_chd_disk,
    index::DatDiskIndex,
    limits::DatLimits,
    model::ParsedDat,
    parsers::parse_dat_file,
    rename_plan::{RenamePlanContext, build_rename_plan},
    sources::{
        DatSourceKind,
        audit_run::{DatAuditRequest, run_dat_audit},
    },
};
use crate::safe_read::TrustedRoots;
use std::sync::atomic::AtomicBool;

const SHA: &str = "1111111111111111111111111111111111111111";

struct Fixture {
    _root: tempfile::TempDir,
    dat: ParsedDat,
    disks: Vec<DatDiskAudit>,
    request: DatAuditRequest,
}

impl Fixture {
    fn new(xml: &str, image_parent: Option<u8>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let dat_path = root.path().join("fixture.xml");
        std::fs::write(&dat_path, xml.replace("SHA", SHA)).unwrap();
        let dat = parse_dat_file(&dat_path, DatLimits::default()).unwrap().dat;
        let scan_root = root.path().join("library");
        std::fs::create_dir(&scan_root).unwrap();
        let image = scan_root.join("fixture.chd");
        let mut header = vec![0; 124];
        header[..8].copy_from_slice(b"MComprHD");
        header[8..12].copy_from_slice(&124_u32.to_be_bytes());
        header[12..16].copy_from_slice(&5_u32.to_be_bytes());
        header[32..40].copy_from_slice(&4096_u64.to_be_bytes());
        header[56..60].copy_from_slice(&131072_u32.to_be_bytes());
        header[60..64].copy_from_slice(&2048_u32.to_be_bytes());
        header[84..104].fill(0x11);
        if let Some(parent) = image_parent {
            header[104..124].fill(parent);
        }
        std::fs::write(&image, header).unwrap();
        let disks = vec![audit_chd_disk(
            &image,
            &TrustedRoots::none(),
            &DatDiskIndex::build(&dat),
        )];
        let request = DatAuditRequest {
            source_id: "fixture".into(),
            source_display_name: "Disposable catalogue".into(),
            dat_path,
            dat_kind: DatSourceKind::File,
            scan_root,
            limits: DatLimits::default(),
            policy: None,
            platform: None,
        };
        Self {
            _root: root,
            dat,
            disks,
            request,
        }
    }

    fn resolve(&self, complete: bool) -> Vec<SetResolution> {
        let mut sets = classify_disk_only_sets(&self.disks, complete, &self.dat.games, "fixture");
        let evidence = CollectionEvidence::build(&[], &self.disks, &self.dat.games, complete);
        resolve_collection(&mut sets, &self.dat.games, &evidence);
        sets
    }
}

fn catalogue(children: &str) -> String {
    format!(
        r#"<datafile><header><name>Disposable</name></header>
        <game name="provider"><disk name="disc" sha1="SHA"/></game>{children}</datafile>"#
    )
}

fn child<'a>(sets: &'a [SetResolution], name: &str) -> &'a SetResolution {
    sets.iter()
        .find(|s| s.identity.game_name == name)
        .unwrap_or_else(|| panic!("missing {name}: {sets:?}"))
}

#[test]
fn borrowed_disk_only_emission_reaches_real_audit_without_roms_or_invented_archive() {
    let f = Fixture::new(
        &catalogue(
            r#"<game name="child" romof="provider">
        <disk name="borrowed" merge="disc"/></game>"#,
        ),
        None,
    );
    let image = f.disks[0].chd_path.clone();
    let before = std::fs::read(&image).unwrap();
    let cancel = AtomicBool::new(false);
    let outcome = run_dat_audit(&f.request, &TrustedRoots::none(), &cancel, &|_| {}).unwrap();
    let borrowed = child(&outcome.sets, "child");
    assert_eq!(borrowed.state, SetState::Complete);
    assert_eq!(borrowed.dependencies.state, DependencyState::Satisfied);
    assert_eq!(borrowed.members_borrowed, ["borrowed"]);
    assert!(borrowed.members_required.is_empty() && borrowed.members_verified.is_empty());
    assert!(borrowed.disks_required.is_empty() && borrowed.disks_identity_matched.is_empty());
    assert!(borrowed.archive_path.as_os_str().is_empty());
    assert!(outcome.archives.is_empty());
    assert!(
        build_rename_plan(&outcome, &RenamePlanContext { generation: 1 }, &cancel)
            .unwrap()
            .proposals
            .is_empty()
    );
    assert_eq!(std::fs::read(image).unwrap(), before);
}

#[test]
fn borrowed_disk_only_chains_and_clone_fallback_follow_existing_provider_policy() {
    let f = Fixture::new(
        &catalogue(
            r#"
        <game name="middle" romof="provider"><disk name="middle-disc" merge="disc"/></game>
        <game name="child" cloneof="middle"><disk name="child-disc" merge="middle-disc"/></game>
        <game name="wrong" romof="absent" cloneof="provider"><disk name="wrong-disc" merge="disc"/></game>
        <game name="empty-romof" romof=" " cloneof="provider"><disk name="bad" merge="disc"/></game>
        "#,
        ),
        None,
    );
    let sets = f.resolve(true);
    assert_eq!(child(&sets, "child").state, SetState::Complete);
    assert!(
        !sets
            .iter()
            .any(|s| matches!(s.identity.game_name.as_str(), "wrong" | "empty-romof"))
    );
}

#[test]
fn borrowed_disk_only_uses_catalogue_ids_and_refuses_name_id_collisions() {
    let xml =
        catalogue(r#"<game name="child" romof="001"><disk name="borrowed" merge="disc"/></game>"#)
            .replace("name=\"provider\"", "name=\"provider\" id=\"001\"");
    let f = Fixture::new(&xml, None);
    assert_eq!(child(&f.resolve(true), "child").state, SetState::Complete);
    let collision = xml.replace("</datafile>", r#"<game name="001"></game></datafile>"#);
    let f = Fixture::new(&collision, None);
    assert!(f.dat.games.iter().any(|game| game.name == "001"));
    assert!(
        !f.resolve(true)
            .iter()
            .any(|s| s.identity.game_name == "child")
    );
}

#[test]
fn borrowed_disk_only_software_list_diskareas_preserve_positional_inheritance() {
    let f = Fixture::new(
        r#"<softwarelist name="disposable">
        <software name="provider"><description>Provider</description><part name="cdrom" interface="cdrom">
        <diskarea name="cdrom"><disk name="disc" sha1="SHA"/></diskarea></part></software>
        <software name="child" cloneof="provider"><description>Child</description><part name="cdrom" interface="cdrom">
        <diskarea name="cdrom"><disk name="borrowed" merge="disc"/></diskarea></part></software>
        </softwarelist>"#,
        None,
    );
    let sets = f.resolve(true);
    assert_eq!(child(&sets, "child").state, SetState::Complete);
    assert_eq!(child(&sets, "child").members_borrowed, ["borrowed"]);
}

#[test]
fn borrowed_disk_only_does_not_invent_empty_clone_device_or_mixed_rom_sets() {
    let f = Fixture::new(
        &catalogue(
            r#"
        <game name="empty" cloneof="provider"></game>
        <game name="own" cloneof="provider"><disk name="own-disc" sha1="2222222222222222222222222222222222222222"/></game>
        <game name="device-only"><device_ref name="provider"/><disk name="borrowed" merge="disc"/></game>
        <game name="mixed" romof="provider"><rom name="needed" size="1" sha1="SHA"/><disk name="borrowed" merge="disc"/></game>
        <game name="cycle-a" romof="cycle-b"><disk name="a" merge="b"/></game>
        <game name="cycle-b" romof="cycle-a"><disk name="b" merge="a"/></game>
        "#,
        ),
        None,
    );
    for name in ["empty", "own", "device-only", "mixed", "cycle-a", "cycle-b"] {
        assert!(f.dat.games.iter().any(|game| game.name == name));
    }
    let sets = f.resolve(true);
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].identity.game_name, "provider");
    assert!(classify_disk_only_sets(&[], true, &f.dat.games, "fixture").is_empty());
}

#[test]
fn borrowed_disk_only_invalid_merge_and_missing_chd_parent_never_complete() {
    for (disk, parent) in [
        (r#"<disk name="borrowed" merge="absent"/>"#, None),
        (
            r#"<disk name="borrowed" merge="disc" sha1="2222222222222222222222222222222222222222"/>"#,
            None,
        ),
        (r#"<disk name="borrowed" merge="disc"/>"#, Some(0x22)),
    ] {
        let f = Fixture::new(
            &catalogue(&format!(
                r#"<game name="child" romof="provider">{disk}</game>"#
            )),
            parent,
        );
        assert_ne!(child(&f.resolve(true), "child").state, SetState::Complete);
    }
}

#[test]
fn borrowed_disk_only_partial_scan_keeps_proven_positives_and_tainted_evidence_fails_closed() {
    let mut f = Fixture::new(
        &catalogue(
            r#"<game name="child" romof="provider"><disk name="borrowed" merge="disc"/></game>"#,
        ),
        None,
    );
    let sets = f.resolve(false);
    assert_eq!(child(&sets, "child").state, SetState::Complete);
    assert_ne!(child(&sets, "provider").state, SetState::Complete);
    let duplicate = audit_chd_disk(
        &f.disks[0].chd_path,
        &TrustedRoots::none(),
        &DatDiskIndex::build(&f.dat),
    );
    f.disks.push(duplicate);
    assert_ne!(child(&f.resolve(true), "child").state, SetState::Complete);
}

#[test]
fn borrowed_disk_only_stale_refs_and_missing_devices_never_complete() {
    let mut f = Fixture::new(
        &catalogue(
            r#"<game name="child" romof="provider"><disk name="borrowed" merge="disc"/></game>"#,
        ),
        None,
    );
    f.disks[0].matched_refs[0].disk_name = "stale".into();
    assert_ne!(child(&f.resolve(true), "child").state, SetState::Complete);
    let f = Fixture::new(
        &catalogue(
            r#"<game name="child" romof="provider"><device_ref name="missing-device"/><disk name="borrowed" merge="disc"/></game>"#,
        ),
        None,
    );
    assert_ne!(child(&f.resolve(true), "child").state, SetState::Complete);
}
