use super::*;
use crate::dat::{
    managed_sources::{ManagedDatSources, save_managed_dat_sources_to},
    updates::{
        ManagedDatSnapshot, ManagedDatSourceDescriptor, ManagedDatState, ManagedDatUpdatePolicy,
        save_managed_dat_state,
    },
};
use sha2::{Digest, Sha256};
struct Fixture {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    config: std::path::PathBuf,
    object: std::path::PathBuf,
    registry: DatSourceRegistry,
    managed: ManagedDatSources,
}
impl Fixture {
    fn new(policy: ManagedDatUpdatePolicy) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("managed");
        let config = dir.path().join("managed.toml");
        let body = r#"<softwarelist name="nes"><software name="fixture"><description>Fixture</description><year>1990</year><publisher>Test</publisher><part name="cart" interface="cart"><dataarea name="rom" size="1"><rom name="fixture.nes" size="1" crc="00000000"/></dataarea></part></software></softwarelist>"#;
        let descriptor = ManagedDatSourceDescriptor::mame_software_list("nes")
            .unwrap()
            .with_update_policy(policy);
        let sha: String = Sha256::digest(body.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let state =
            ManagedDatState::new(&descriptor, ManagedDatSnapshot::new(&sha).unwrap()).unwrap();
        let object = root
            .join(state.source_id.storage_relative_path())
            .join("objects")
            .join(&sha);
        std::fs::create_dir_all(object.parent().unwrap()).unwrap();
        std::fs::write(&object, body).unwrap();
        save_managed_dat_state(&root, &state).unwrap();
        let mut managed = ManagedDatSources::new();
        managed.add_mame_software_list("nes", policy).unwrap();
        save_managed_dat_sources_to(&config, &managed).unwrap();
        Self {
            _dir: dir,
            root,
            config,
            object,
            registry: DatSourceRegistry::new(),
            managed,
        }
    }
    fn load(
        &self,
        platforms: &[String],
        slots: usize,
        bytes: u64,
    ) -> Result<Vec<SpectrumCatalogue>, String> {
        load_from(
            CatalogueInventoryInputs {
                local_registry: &self.registry,
                managed_sources: &self.managed,
                managed_root: &self.root,
                limits: DatLimits {
                    max_file_size: 32 * 1024 * 1024,
                    ..Default::default()
                },
            },
            &FileFingerprint::observe(&self.config).unwrap(),
            platforms,
            slots,
            bytes,
        )
    }
}
#[test]
fn trusted_managed_comparison_uses_shared_resolution_and_retains_exact_input_guards() {
    let f = Fixture::new(ManagedDatUpdatePolicy::Manual);
    let rows = f.load(&["NES".into()], 32, 128 * 1024 * 1024).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].platform, "NES");
    assert!(rows[0].basis.contains("managed:mame:nes"));
    assert!(rows[0].inputs.iter().any(|(p, _)| p == &f.config));
    assert!(rows[0].inputs.iter().any(|(p, _)| p == &f.object));
    assert_eq!(rows[0].inputs.len(), 3);
    std::fs::write(&f.config, "changed").unwrap();
    assert!(
        rows[0]
            .inputs
            .iter()
            .any(|(p, g)| FileFingerprint::observe(p).as_ref() != Some(g))
    );
}
#[test]
fn unrelated_managed_platform_does_not_consume_preview_budget() {
    let f = Fixture::new(ManagedDatUpdatePolicy::Manual);
    assert!(f.load(&["Amiga".into()], 0, 0).unwrap().is_empty());
}
#[test]
fn disabled_managed_catalogue_cannot_supply_comparison_evidence() {
    let f = Fixture::new(ManagedDatUpdatePolicy::Disabled);
    assert!(f.load(&[], 32, u64::MAX).unwrap().is_empty());
}
#[test]
fn stale_or_tampered_snapshot_cannot_supply_comparison_evidence() {
    let f = Fixture::new(ManagedDatUpdatePolicy::Manual);
    std::fs::write(&f.object, b"not the snapshot").unwrap();
    let result = f.load(&[], 32, u64::MAX);
    assert!(result.is_err() || result.unwrap().is_empty());
}
#[test]
fn managed_inputs_share_existing_count_and_byte_limits() {
    let f = Fixture::new(ManagedDatUpdatePolicy::Manual);
    assert!(f.load(&[], 0, u64::MAX).is_err());
    assert!(f.load(&[], 32, 1).is_err());
}

#[test]
fn descriptor_loaded_before_a_policy_change_cannot_bind_the_new_configuration() {
    let f = Fixture::new(ManagedDatUpdatePolicy::Manual);
    let guard = FileFingerprint::observe(&f.config).unwrap();
    std::fs::write(&f.config, "policy changed after descriptor load").unwrap();
    let result = load_from(
        CatalogueInventoryInputs {
            local_registry: &f.registry,
            managed_sources: &f.managed,
            managed_root: &f.root,
            limits: DatLimits::default(),
        },
        &guard,
        &[],
        32,
        u64::MAX,
    );
    assert!(result.is_err());
}
